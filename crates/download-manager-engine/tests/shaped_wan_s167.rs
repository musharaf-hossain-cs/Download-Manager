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
    benchmark_runner::{
        BenchmarkApplication, BenchmarkCommand, BenchmarkRole, PairedBenchmarkPlan,
        run_paired_benchmark,
    },
    benchmark_wan::{PairedTransferCounters, TransferCounters, WanShape, build_20_80ms_wan_report},
};

const PAIRS: u32 = 10;

#[test]
fn shaped_20_80ms_runs_report_paired_throughput_retries_and_wire_bytes() {
    let temporary = tempfile::tempdir().expect("temporary benchmark directory");
    let mut profiles = Vec::new();
    for rtt_ms in [20, 80] {
        profiles.push(run_profile(temporary.path(), rtt_ms));
    }

    let report = build_20_80ms_wan_report(profiles).expect("valid shaped-WAN evidence");
    assert_eq!(report.profiles.len(), 2);
    assert_eq!(report.profiles[0].shape.rtt_ms, 20);
    assert_eq!(report.profiles[1].shape.rtt_ms, 80);
    for profile in &report.profiles {
        assert_eq!(profile.paired_report.samples.len(), PAIRS as usize);
        assert!(profile.paired_report.summary.all_output_hashes_match);
        assert_eq!(profile.retry_count.baseline.median, 0.0);
        assert_eq!(profile.retry_count.candidate.median, 0.0);
        assert!(profile.wire_bytes.baseline.median > 32_768.0);
        assert!(profile.wire_bytes.candidate.median > 32_768.0);
    }

    let json = serde_json::to_vec_pretty(&report).expect("WAN JSON report");
    let markdown = report.render_markdown();
    fs::write(temporary.path().join("wan-20-80.json"), &json).expect("write JSON");
    fs::write(temporary.path().join("wan-20-80.md"), &markdown).expect("write Markdown");
    let parsed: serde_json::Value = serde_json::from_slice(&json).expect("parse JSON");
    assert_eq!(parsed["profiles"].as_array().map(Vec::len), Some(2));
    assert!(markdown.contains("20 ms"));
    assert!(markdown.contains("80 ms"));
    assert!(markdown.contains("Paired goodput"));
    assert!(markdown.contains("Paired retries"));
    assert!(markdown.contains("Paired wire bytes"));
}

fn run_profile(
    directory: &std::path::Path,
    rtt_ms: u16,
) -> (
    WanShape,
    download_manager_engine::benchmark_runner::PairedBenchmarkReport,
    Vec<PairedTransferCounters>,
) {
    let body = Arc::new(
        (0_u32..32 * 1024)
            .map(|value| u8::try_from(value % 251).expect("byte pattern"))
            .collect::<Vec<_>>(),
    );
    let observations = Arc::new(Mutex::new(BTreeMap::new()));
    let (source, server) =
        start_shaped_server(rtt_ms, Arc::clone(&body), Arc::clone(&observations));
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
        scenario: format!("controlled-shaped-wan-{rtt_ms}ms"),
        protocol: "HTTP/1.1".to_owned(),
        seed: 0x51_67_00 + u64::from(rtt_ms),
        pairs: PAIRS,
        source: format!("{source}?pair={{pair}}&seed={{seed}}"),
        output_directory: directory.join(format!("outputs-{rtt_ms}")),
        retain_outputs: false,
        baseline: application("IDM fixture", "baseline"),
        candidate: application("Download Manager fixture", "candidate"),
    })
    .expect("paired shaped-WAN benchmark");
    server.join().expect("shaped-WAN server");

    let observations = observations.lock().expect("observation lock");
    let counters = (1..=PAIRS)
        .map(|pair| PairedTransferCounters {
            pair,
            baseline: observations[&(pair, BenchmarkRole::Baseline)],
            candidate: observations[&(pair, BenchmarkRole::Candidate)],
        })
        .collect();
    (
        WanShape {
            rtt_ms,
            loss_basis_points: 10,
            bandwidth_mbps: 100,
        },
        report,
        counters,
    )
}

fn start_shaped_server(
    rtt_ms: u16,
    body: Arc<Vec<u8>>,
    observations: Arc<Mutex<BTreeMap<(u32, BenchmarkRole), TransferCounters>>>,
) -> (String, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind shaped-WAN fixture");
    let address = listener.local_addr().expect("fixture address");
    let server = thread::spawn(move || {
        let expected_requests = PAIRS * 2 + 1;
        for connection in listener.incoming().take(expected_requests as usize) {
            let mut stream = connection.expect("accept shaped request");
            let request = read_request(&mut stream);
            thread::sleep(Duration::from_millis(u64::from(rtt_ms)));
            let should_retry = request.pair == 1
                && request.role == BenchmarkRole::Candidate
                && !observations
                    .lock()
                    .expect("observation lock")
                    .contains_key(&(request.pair, request.role));
            if should_retry {
                let response = b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
                stream.write_all(response).expect("retry response");
                observations.lock().expect("observation lock").insert(
                    (request.pair, request.role),
                    TransferCounters {
                        retry_count: 1,
                        wire_bytes: response.len() as u64,
                    },
                );
                continue;
            }
            let headers = format!(
                "HTTP/1.1 206 Partial Content\r\nContent-Length: {}\r\nContent-Range: bytes 0-{}/{}\r\nConnection: close\r\n\r\n",
                body.len(),
                body.len() - 1,
                body.len()
            );
            stream
                .write_all(headers.as_bytes())
                .expect("response headers");
            stream.write_all(&body).expect("response body");
            let mut observations = observations.lock().expect("observation lock");
            let counters = observations
                .entry((request.pair, request.role))
                .or_default();
            counters.wire_bytes += (headers.len() + body.len()) as u64;
        }
    });
    (format!("http://{address}/fixture.bin"), server)
}

#[derive(Debug)]
struct Request {
    pair: u32,
    role: BenchmarkRole,
}

fn read_request(stream: &mut TcpStream) -> Request {
    let mut bytes = Vec::new();
    let mut buffer = [0_u8; 1024];
    while !bytes.windows(4).any(|window| window == b"\r\n\r\n") {
        let read = stream.read(&mut buffer).expect("request bytes");
        assert!(read > 0, "client closed before request headers");
        bytes.extend_from_slice(&buffer[..read]);
    }
    let text = String::from_utf8(bytes).expect("ASCII request");
    let request_line = text.lines().next().expect("request line");
    let target = request_line.split_whitespace().nth(1).expect("target");
    let query = target.split_once('?').expect("query").1;
    let pair = query
        .split('&')
        .find_map(|part| part.strip_prefix("pair="))
        .expect("pair")
        .parse()
        .expect("numeric pair");
    let role = text
        .lines()
        .find_map(|line| {
            line.to_ascii_lowercase()
                .strip_prefix("x-benchmark-role: ")
                .map(str::to_owned)
        })
        .map(|role| {
            if role == "baseline" {
                BenchmarkRole::Baseline
            } else {
                BenchmarkRole::Candidate
            }
        })
        .expect("benchmark role");
    Request { pair, role }
}
