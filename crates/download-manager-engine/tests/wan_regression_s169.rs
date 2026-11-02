use std::{
    collections::HashMap,
    fs,
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    path::PathBuf,
    sync::{Arc, Mutex},
    thread,
    time::Duration,
};

use download_manager_engine::{
    benchmark_runner::{
        BenchmarkApplication, BenchmarkCommand, PairedBenchmarkPlan, run_paired_benchmark,
    },
    benchmark_wan_regression::{
        FairnessEvidence, WAN_POLL_REGRESSION_SCENARIO, WAN_REGRESSION_RTT_MILLIS,
        WanRegressionReportError, build_wan_regression_report,
    },
    domain::{JobId, Priority},
    fairness::WeightedFairScheduler,
};

const PAIRS: u32 = 10;
const BODY_BYTES: usize = 128 * 1024;

#[test]
fn fixed_receive_polling_regression_is_measured_fixed_and_fair() {
    let body = Arc::new(
        (0..BODY_BYTES)
            .map(|value| u8::try_from(value % 251).expect("byte pattern"))
            .collect::<Vec<_>>(),
    );
    let requests = Arc::new(Mutex::new(Vec::new()));
    let (source, server) = start_server(Arc::clone(&body), Arc::clone(&requests));
    let temporary = tempfile::tempdir().expect("temporary benchmark directory");
    let client = PathBuf::from(env!("CARGO_BIN_EXE_download-manager-wan-benchmark-client"));
    let application = |name: &str, command: &str, role: &str| BenchmarkApplication {
        name: name.to_owned(),
        version_command: BenchmarkCommand {
            program: client.clone(),
            arguments: vec!["--version".to_owned()],
        },
        download_command: BenchmarkCommand {
            program: client.clone(),
            arguments: vec![
                command.to_owned(),
                role.to_owned(),
                "{url}".to_owned(),
                "{output}".to_owned(),
            ],
        },
    };
    let paired = run_paired_benchmark(&PairedBenchmarkPlan {
        schema_version: 1,
        scenario: WAN_POLL_REGRESSION_SCENARIO.to_owned(),
        protocol: "HTTP/1.1".to_owned(),
        seed: 0x51_69_01,
        pairs: PAIRS,
        source: format!("{source}?pair={{pair}}&seed={{seed}}"),
        output_directory: temporary.path().join("outputs"),
        retain_outputs: false,
        baseline: application("before: fixed polling delay", "profile-before", "baseline"),
        candidate: application(
            "after: readiness-driven reads",
            "profile-after",
            "candidate",
        ),
    })
    .expect("paired WAN regression benchmark");
    server.join().expect("WAN fixture server");

    let fairness = weighted_fairness_sample(700);
    let evidence =
        build_wan_regression_report(&paired, fairness).expect("improved and fair WAN evidence");
    eprintln!(
        "S169 median paired goodput improvement: {:+.2}%",
        evidence.median_goodput_improvement_percent
    );
    assert!(evidence.median_goodput_improvement_percent > 0.0);
    assert_eq!(evidence.fairness.high_selections, 400);
    assert_eq!(evidence.fairness.normal_selections, 200);
    assert_eq!(evidence.fairness.low_selections, 100);
    assert!(evidence.fairness.longest_low_priority_gap <= 6);
    assert!(evidence.weighted_fairness_preserved);
    assert!(evidence.correctness_preserved);
    assert!(paired.samples.iter().all(|sample| {
        sample.baseline.bytes == BODY_BYTES as u64
            && sample.candidate.bytes == BODY_BYTES as u64
            && sample.output_hash_matches
    }));

    let captured = requests.lock().expect("captured request lock");
    assert_eq!(captured.len(), (PAIRS * 2) as usize);
    assert!(
        captured
            .iter()
            .all(|request| request.range.as_deref() == Some("bytes=0-"))
    );
    assert_eq!(
        captured
            .iter()
            .filter(|request| request.role == "baseline")
            .count(),
        PAIRS as usize
    );
    assert_eq!(
        captured
            .iter()
            .filter(|request| request.role == "candidate")
            .count(),
        PAIRS as usize
    );

    assert_report_artifacts(temporary.path(), &evidence);

    let unfair = FairnessEvidence {
        low_selections: 99,
        ..fairness
    };
    assert_eq!(
        build_wan_regression_report(&paired, unfair),
        Err(WanRegressionReportError::FairnessRegression)
    );
}

fn assert_report_artifacts(
    directory: &std::path::Path,
    evidence: &download_manager_engine::benchmark_wan_regression::WanRegressionReport,
) {
    let json = serde_json::to_vec_pretty(evidence).expect("JSON regression report");
    let markdown = evidence.render_markdown();
    let json_path = directory.join("wan-regression.json");
    let markdown_path = directory.join("wan-regression.md");
    fs::write(&json_path, json).expect("write JSON regression report");
    fs::write(&markdown_path, &markdown).expect("write Markdown regression report");
    let parsed: serde_json::Value =
        serde_json::from_slice(&fs::read(json_path).expect("read JSON regression report"))
            .expect("parse JSON regression report");
    assert_eq!(parsed["rtt_millis"], u64::from(WAN_REGRESSION_RTT_MILLIS));
    assert_eq!(parsed["before_fixed_poll_delay_millis"], 20);
    assert!(markdown.contains("Fixed receive poll delay | 20 ms | 0 ms"));
    assert!(markdown.contains("High / normal / low selections | 400 / 200 / 100"));
    assert!(markdown.contains("Correctness preserved: **true**"));
}

#[test]
fn fairness_suite_covers_equal_priority_and_pressure_transitions() {
    let jobs = [JobId::new(), JobId::new(), JobId::new()];
    let mut equal = WeightedFairScheduler::default();
    for id in jobs {
        equal.register(id, Priority::Normal).expect("equal job");
    }
    let mut counts = HashMap::new();
    for _ in 0..300 {
        *counts
            .entry(equal.next_job().expect("active equal job"))
            .or_insert(0_u64) += 1;
    }
    assert!(jobs.into_iter().all(|id| counts[&id] == 100));

    let high = JobId::new();
    let normal = JobId::new();
    let low = JobId::new();
    let mut pressured = WeightedFairScheduler::default();
    pressured.register(high, Priority::High).expect("high");
    pressured
        .register(normal, Priority::Normal)
        .expect("normal");
    pressured.register(low, Priority::Low).expect("low");
    for _ in 0..5 {
        pressured.next_job().expect("warm fairness cycle");
    }
    pressured.set_active(high, false).expect("pause high");
    assert!((0..3).any(|_| pressured.next_job() == Some(low)));
    pressured.set_active(high, true).expect("resume high");
    let resumed = (0..14)
        .map(|_| pressured.next_job().expect("resumed fairness job"))
        .collect::<Vec<_>>();
    assert!(resumed.contains(&high));
    assert!(resumed.contains(&normal));
    assert!(resumed.contains(&low));
}

fn weighted_fairness_sample(selections: u64) -> FairnessEvidence {
    let high = JobId::new();
    let normal = JobId::new();
    let low = JobId::new();
    let mut scheduler = WeightedFairScheduler::default();
    scheduler.register(high, Priority::High).expect("high");
    scheduler
        .register(normal, Priority::Normal)
        .expect("normal");
    scheduler.register(low, Priority::Low).expect("low");

    let mut evidence = FairnessEvidence {
        selections,
        high_selections: 0,
        normal_selections: 0,
        low_selections: 0,
        longest_low_priority_gap: 0,
    };
    let mut since_low = 0;
    for _ in 0..selections {
        let selected = scheduler.next_job().expect("active weighted job");
        if selected == high {
            evidence.high_selections += 1;
            since_low += 1;
        } else if selected == normal {
            evidence.normal_selections += 1;
            since_low += 1;
        } else {
            evidence.low_selections += 1;
            evidence.longest_low_priority_gap = evidence.longest_low_priority_gap.max(since_low);
            since_low = 0;
        }
    }
    evidence
}

#[derive(Debug)]
struct CapturedRequest {
    range: Option<String>,
    role: String,
}

fn start_server(
    body: Arc<Vec<u8>>,
    requests: Arc<Mutex<Vec<CapturedRequest>>>,
) -> (String, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind WAN fixture");
    let address = listener.local_addr().expect("fixture address");
    let server = thread::spawn(move || {
        for connection in listener.incoming().take((PAIRS * 2) as usize) {
            let mut stream = connection.expect("accept WAN request");
            let request = read_request(&mut stream);
            requests.lock().expect("request lock").push(request);
            thread::sleep(Duration::from_millis(u64::from(WAN_REGRESSION_RTT_MILLIS)));
            write!(
                stream,
                "HTTP/1.1 206 Partial Content\r\nContent-Length: {}\r\nContent-Range: bytes 0-{}/{}\r\nConnection: close\r\n\r\n",
                body.len(),
                body.len() - 1,
                body.len(),
            )
            .expect("response headers");
            for chunk in body.chunks(8 * 1024) {
                stream.write_all(chunk).expect("response body chunk");
                thread::sleep(Duration::from_millis(1));
            }
        }
    });
    (format!("http://{address}/fixture.bin"), server)
}

fn read_request(stream: &mut TcpStream) -> CapturedRequest {
    let mut bytes = Vec::new();
    let mut buffer = [0_u8; 1024];
    while !bytes.windows(4).any(|window| window == b"\r\n\r\n") {
        let read = stream.read(&mut buffer).expect("request bytes");
        assert!(read > 0, "client closed before request headers");
        bytes.extend_from_slice(&buffer[..read]);
        assert!(bytes.len() <= 64 * 1024, "request headers are bounded");
    }
    let text = String::from_utf8(bytes).expect("ASCII request");
    let header = |expected: &str| {
        text.lines().find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case(expected)
                .then(|| value.trim().to_owned())
        })
    };
    CapturedRequest {
        range: header("range"),
        role: header("x-benchmark-role").expect("benchmark role"),
    }
}
