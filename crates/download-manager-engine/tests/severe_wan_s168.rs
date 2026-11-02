use std::{
    collections::BTreeMap,
    fs,
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    path::PathBuf,
    sync::{Arc, Mutex},
    thread,
    time::Duration,
};

use download_manager_engine::{
    adaptive::{AutoConcurrencyController, ConcurrencySample, PressureSample},
    benchmark_runner::{
        BenchmarkApplication, BenchmarkCommand, PairedBenchmarkPlan, PairedBenchmarkReport,
        run_paired_benchmark,
    },
    benchmark_wan_severe::{
        AdaptiveControllerStep, AdaptivePairTrace, SEVERE_WAN_SCENARIO, SevereWanProfile,
        SevereWanReportError, build_severe_wan_report,
    },
};

const PAIRS: u32 = 10;
const INTERVAL_BYTES_PER_CONNECTION: u64 = 512 * 1024;
const BODY_BYTES: usize = 64 * 1024;

#[test]
fn severe_wan_reports_paired_adaptive_backoff_and_recovery() {
    let paired = paired_report();
    let traces = (1..=PAIRS).map(controller_trace).collect::<Vec<_>>();
    let evidence = build_severe_wan_report(&paired, SevereWanProfile::default(), traces)
        .expect("valid severe WAN evidence");

    assert!(evidence.adaptive_behavior_valid);
    assert_eq!(evidence.controller_summary.pairs_backed_off, PAIRS as usize);
    assert_eq!(evidence.controller_summary.pairs_recovered, PAIRS as usize);
    assert_eq!(evidence.controller_summary.loss_events, 5);
    assert_eq!(evidence.controller_summary.throttle_events, 5);
    assert_eq!(evidence.controller_summary.retries, u64::from(PAIRS));
    assert!(evidence.controller_summary.wire_bytes > evidence.controller_summary.useful_bytes);
    assert!(evidence.controller_summary.maximum_target_connections >= 8);

    let temporary = tempfile::tempdir().expect("temporary report directory");
    let json_path = temporary.path().join("wan-180ms-loss-throttle.json");
    let markdown_path = temporary.path().join("wan-180ms-loss-throttle.md");
    let json = serde_json::to_vec_pretty(&evidence).expect("serialize severe WAN evidence");
    let markdown = evidence.render_markdown();
    fs::write(&json_path, json).expect("write JSON evidence");
    fs::write(&markdown_path, &markdown).expect("write Markdown evidence");

    let parsed = serde_json::from_slice::<serde_json::Value>(
        &fs::read(json_path).expect("read severe WAN evidence"),
    )
    .expect("parse severe WAN evidence");
    assert_eq!(parsed["profile"]["rtt_millis"], 180);
    assert_eq!(parsed["profile"]["loss_basis_points"], 100);
    assert_eq!(
        parsed["controller_traces"].as_array().map(Vec::len),
        Some(PAIRS as usize)
    );
    assert!(markdown.contains("RTT: **180 ms**; loss: **1.00%**"));
    assert!(markdown.contains("Pairs backed off / recovered | 10 / 10"));
    assert!(markdown.contains("Adaptive-controller gate: **PASS**"));
}

#[test]
fn severe_wan_rejects_trace_without_recovery() {
    let paired = paired_report();
    let mut traces = (1..=PAIRS).map(controller_trace).collect::<Vec<_>>();
    traces[0].steps.truncate(4);

    assert_eq!(
        build_severe_wan_report(&paired, SevereWanProfile::default(), traces),
        Err(SevereWanReportError::MissingAdaptiveResponse { pair: 1 })
    );
}

fn controller_trace(pair: u32) -> AdaptivePairTrace {
    let mut controller = AutoConcurrencyController::new(32).expect("adaptive controller");
    let mut steps = Vec::new();
    for interval in 1..=3 {
        steps.push(productive_step(interval, &mut controller));
    }

    let target_before = controller.target();
    let loss = pair % 2 == 1;
    let decision = controller.observe_pressure(PressureSample {
        retry_count: 1,
        throttled: !loss,
        network_loss: loss,
        pending_disk_writes: 0,
    });
    steps.push(AdaptiveControllerStep {
        interval: 4,
        target_before,
        target_after: decision.target,
        useful_bytes: 0,
        wire_bytes: 16 * 1024,
        retries: 1,
        throttled: !loss,
        network_loss: loss,
    });
    for interval in 5..=7 {
        steps.push(productive_step(interval, &mut controller));
    }
    AdaptivePairTrace { pair, steps }
}

fn productive_step(
    interval: u32,
    controller: &mut AutoConcurrencyController,
) -> AdaptiveControllerStep {
    let target_before = controller.target();
    let useful_bytes = INTERVAL_BYTES_PER_CONNECTION * u64::from(target_before);
    let target_after = controller.observe_productive_interval(ConcurrencySample {
        active_connections: target_before,
        completed_bytes: useful_bytes,
        elapsed_millis: 1_000,
    });
    AdaptiveControllerStep {
        interval,
        target_before,
        target_after,
        useful_bytes,
        wire_bytes: useful_bytes,
        retries: 0,
        throttled: false,
        network_loss: false,
    }
}

fn paired_report() -> PairedBenchmarkReport {
    let body = Arc::new(
        (0..BODY_BYTES)
            .map(|value| u8::try_from(value % 251).expect("byte pattern"))
            .collect::<Vec<_>>(),
    );
    let requests = Arc::new(Mutex::new(Vec::new()));
    let (source, server) = start_severe_wan_server(Arc::clone(&body), Arc::clone(&requests));
    let temporary = tempfile::tempdir().expect("temporary severe-WAN directory");
    let client = PathBuf::from(env!("CARGO_BIN_EXE_download-manager-wan-benchmark-client"));
    let application = |name: &str, role: &str| BenchmarkApplication {
        name: name.to_owned(),
        version_command: BenchmarkCommand {
            program: client.clone(),
            arguments: vec!["--version".to_owned()],
        },
        download_command: BenchmarkCommand {
            program: client.clone(),
            arguments: vec![
                "download".to_owned(),
                role.to_owned(),
                "{url}".to_owned(),
                "{output}".to_owned(),
            ],
        },
    };
    let report = run_paired_benchmark(&PairedBenchmarkPlan {
        schema_version: 1,
        scenario: SEVERE_WAN_SCENARIO.to_owned(),
        protocol: "HTTP/1.1".to_owned(),
        seed: 0x51_68_01,
        pairs: PAIRS,
        source: format!("{source}?pair={{pair}}&seed={{seed}}"),
        output_directory: temporary.path().join("outputs"),
        retain_outputs: false,
        baseline: application("IDM fixture", "baseline"),
        candidate: application("Download Manager fixture", "candidate"),
    })
    .expect("paired severe-WAN benchmark");
    server.join().expect("severe-WAN fixture server");

    let captured = requests.lock().expect("captured request lock");
    assert_eq!(captured.len(), (PAIRS * 2) as usize);
    let mut per_pair = BTreeMap::<String, usize>::new();
    for request in captured.iter() {
        assert_eq!(request.range.as_deref(), Some("bytes=0-"));
        *per_pair.entry(request.target.clone()).or_default() += 1;
    }
    assert_eq!(per_pair.len(), PAIRS as usize);
    assert!(per_pair.values().all(|count| *count == 2));
    assert!(report.samples.iter().all(|sample| {
        sample.baseline.bytes == BODY_BYTES as u64
            && sample.candidate.bytes == BODY_BYTES as u64
            && sample.output_hash_matches
    }));
    report
}

#[derive(Debug)]
struct CapturedRequest {
    target: String,
    range: Option<String>,
}

fn start_severe_wan_server(
    body: Arc<Vec<u8>>,
    requests: Arc<Mutex<Vec<CapturedRequest>>>,
) -> (String, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind severe-WAN fixture");
    let address = listener.local_addr().expect("severe-WAN address");
    let server = thread::spawn(move || {
        for connection in listener.incoming().take((PAIRS * 2) as usize) {
            let mut stream = connection.expect("accept severe-WAN request");
            requests
                .lock()
                .expect("request lock")
                .push(read_request(&mut stream));
            thread::sleep(Duration::from_millis(180));
            let headers = format!(
                "HTTP/1.1 206 Partial Content\r\nContent-Length: {}\r\nContent-Range: bytes 0-{}/{}\r\nConnection: close\r\n\r\n",
                body.len(),
                body.len() - 1,
                body.len()
            );
            stream
                .write_all(headers.as_bytes())
                .expect("response headers");
            for chunk in body.chunks(8 * 1024) {
                stream.write_all(chunk).expect("throttled response chunk");
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
    let request_line = text.lines().next().expect("request line");
    let target = request_line
        .split_whitespace()
        .nth(1)
        .expect("request target")
        .to_owned();
    let range = text.lines().find_map(|line| {
        let (name, value) = line.split_once(':')?;
        name.eq_ignore_ascii_case("range")
            .then(|| value.trim().to_owned())
    });
    CapturedRequest { target, range }
}
