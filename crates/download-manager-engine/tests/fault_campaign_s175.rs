use std::{
    fs::{self, OpenOptions},
    io::{Seek, SeekFrom, Write},
};

use download_manager_engine::{
    media_worker::{
        FfmpegContainer, FfmpegMuxRequest, FfmpegWorker, MediaWorkerError, MediaWorkerLimits,
    },
    recovery::{ReconciliationAction, reconcile_partial},
    route_reconnect::{ConnectionCheckpoint, RouteChangeOutcome, RouteReconnectCoordinator},
    segmented::ByteRange,
};
use sha2::{Digest, Sha256};

const RUNS: u32 = 50;
const SEED: u64 = 0x5175_fa17_cafe_beef;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InjectedFault {
    ProcessKill,
    WorkerCrash,
    NetworkRouteLoss,
}

#[derive(Debug)]
struct RunEvidence {
    run: u32,
    seed: u64,
    fault: InjectedFault,
    completion_claimed: bool,
    expected_sha256: String,
    actual_sha256: Option<String>,
}

#[test]
fn fifty_seeded_process_worker_network_faults_never_mark_corrupt_output_complete() {
    let directory = tempfile::tempdir().expect("fault campaign directory");
    let worker_executable = std::env::current_exe().expect("current test executable");
    let worker = FfmpegWorker::new(worker_executable, MediaWorkerLimits::default())
        .expect("incompatible fixture worker");
    let mut random = SEED;
    let mut evidence = Vec::with_capacity(RUNS as usize);

    for run in 0..RUNS {
        random = next_seed(random);
        let payload = seeded_payload(random, 48 * 1024 + bounded_index(random, 48 * 1024));
        let expected_sha256 = sha256(&payload);
        let fault = match run % 3 {
            0 => InjectedFault::ProcessKill,
            1 => InjectedFault::WorkerCrash,
            _ => InjectedFault::NetworkRouteLoss,
        };
        let run_directory = directory.path().join(format!("run-{run:02}"));
        fs::create_dir(&run_directory).expect("run directory");

        let result = match fault {
            InjectedFault::ProcessKill => {
                process_kill_run(run, random, &run_directory, &payload, expected_sha256)
            }
            InjectedFault::WorkerCrash => worker_crash_run(
                run,
                random,
                &run_directory,
                &payload,
                expected_sha256,
                &worker,
            ),
            InjectedFault::NetworkRouteLoss => {
                network_route_loss_run(run, random, &run_directory, &payload, expected_sha256)
            }
        };
        evidence.push(result);
    }

    assert_eq!(evidence.len(), RUNS as usize);
    for fault in [
        InjectedFault::ProcessKill,
        InjectedFault::WorkerCrash,
        InjectedFault::NetworkRouteLoss,
    ] {
        assert!(
            evidence.iter().any(|run| run.fault == fault),
            "campaign omitted {fault:?}"
        );
    }
    for run in &evidence {
        if run.completion_claimed {
            assert_eq!(
                run.actual_sha256.as_deref(),
                Some(run.expected_sha256.as_str()),
                "run {} seed {:#x} marked corrupt output complete after {:?}",
                run.run,
                run.seed,
                run.fault
            );
        } else {
            assert!(
                run.actual_sha256.is_none(),
                "failed run {} seed {:#x} exposed an output after {:?}",
                run.run,
                run.seed,
                run.fault
            );
        }
    }
}

fn process_kill_run(
    run: u32,
    seed: u64,
    directory: &std::path::Path,
    payload: &[u8],
    expected_sha256: String,
) -> RunEvidence {
    let output = directory.join("process.part");
    let checkpoint_size = 4096_usize;
    let fault_offset =
        checkpoint_size + 1 + bounded_index(seed, payload.len() - checkpoint_size - 1);
    let durable_offset = fault_offset / checkpoint_size * checkpoint_size;
    fs::write(&output, &payload[..fault_offset]).expect("interrupted process output");

    let reconciliation =
        reconcile_partial(&output, durable_offset as u64).expect("reconcile process interruption");
    assert_eq!(
        reconciliation.action,
        ReconciliationAction::FileTruncatedToDatabase
    );
    let mut resumed = OpenOptions::new()
        .append(true)
        .open(&output)
        .expect("resume process output");
    resumed
        .write_all(&payload[durable_offset..])
        .expect("write missing process suffix");
    resumed.sync_all().expect("sync resumed process output");
    let actual_sha256 = sha256(&fs::read(output).expect("completed process output"));

    RunEvidence {
        run,
        seed,
        fault: InjectedFault::ProcessKill,
        completion_claimed: true,
        expected_sha256,
        actual_sha256: Some(actual_sha256),
    }
}

fn worker_crash_run(
    run: u32,
    seed: u64,
    directory: &std::path::Path,
    payload: &[u8],
    expected_sha256: String,
    worker: &FfmpegWorker,
) -> RunEvidence {
    let input = directory.join("worker-input.bin");
    let output = directory.join("worker-output.mkv");
    fs::write(&input, payload).expect("worker input");
    let result = worker.run(&FfmpegMuxRequest {
        inputs: vec![input.clone()],
        output: output.clone(),
        container: FfmpegContainer::Matroska,
    });
    assert!(
        matches!(
            result,
            Err(MediaWorkerError::Exited { .. } | MediaWorkerError::MissingOutput)
        ),
        "incompatible child unexpectedly succeeded: {result:?}"
    );
    assert_eq!(fs::read(input).expect("preserved worker input"), payload);
    assert!(!output.exists(), "failed worker left a claimable output");

    RunEvidence {
        run,
        seed,
        fault: InjectedFault::WorkerCrash,
        completion_claimed: false,
        expected_sha256,
        actual_sha256: None,
    }
}

fn network_route_loss_run(
    run: u32,
    seed: u64,
    directory: &std::path::Path,
    payload: &[u8],
    expected_sha256: String,
) -> RunEvidence {
    let output = directory.join("network.part");
    let durable_offset = 1 + bounded_index(seed, payload.len() - 1);
    fs::write(&output, vec![0xa5; payload.len()]).expect("preallocate network output");
    write_at(&output, 0, &payload[..durable_offset]);

    let mut coordinator =
        RouteReconnectCoordinator::new("ethernet:campaign").expect("initial campaign route");
    let old_generation = coordinator.generation();
    let checkpoint = [ConnectionCheckpoint {
        connection_id: run,
        range: ByteRange {
            start: 0,
            end: payload.len() as u64 - 1,
        },
        durable_offset: durable_offset as u64,
    }];
    let RouteChangeOutcome::Reconnect(plan) = coordinator
        .observe_route("offline:campaign", &checkpoint)
        .expect("network loss reconnect plan")
    else {
        panic!("changed network route must reconnect");
    };
    assert!(!coordinator.accepts_generation(old_generation));
    assert!(coordinator.accepts_generation(plan.generation));
    let missing = plan.actions[0]
        .resume_range
        .expect("incomplete transfer must have a suffix");
    assert_eq!(missing.start, durable_offset as u64);
    write_at(&output, missing.start, &payload[durable_offset..]);
    let actual_sha256 = sha256(&fs::read(output).expect("completed network output"));

    RunEvidence {
        run,
        seed,
        fault: InjectedFault::NetworkRouteLoss,
        completion_claimed: true,
        expected_sha256,
        actual_sha256: Some(actual_sha256),
    }
}

fn write_at(path: &std::path::Path, offset: u64, bytes: &[u8]) {
    let mut output = OpenOptions::new()
        .write(true)
        .open(path)
        .expect("open ranged output");
    output
        .seek(SeekFrom::Start(offset))
        .expect("seek ranged output");
    output.write_all(bytes).expect("write ranged output");
    output.sync_all().expect("sync ranged output");
}

fn next_seed(seed: u64) -> u64 {
    seed.wrapping_mul(6_364_136_223_846_793_005)
        .wrapping_add(1_442_695_040_888_963_407)
}

fn bounded_index(seed: u64, upper_exclusive: usize) -> usize {
    let upper = u64::try_from(upper_exclusive).expect("fixture bound fits u64");
    usize::try_from(seed % upper).expect("bounded fixture index fits usize")
}

fn seeded_payload(seed: u64, length: usize) -> Vec<u8> {
    let mut random = seed;
    (0..length)
        .map(|_| {
            random = next_seed(random);
            random.to_le_bytes()[3]
        })
        .collect()
}

fn sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
