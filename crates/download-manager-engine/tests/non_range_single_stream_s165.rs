use std::{
    collections::BTreeMap,
    fs,
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    path::PathBuf,
    sync::{Arc, Mutex},
    thread,
};

use download_manager_engine::{
    benchmark_lan::{
        MAXIMUM_SINGLE_STREAM_OVERHEAD_PERCENT, NON_RANGE_SINGLE_STREAM_SCENARIO,
        build_non_range_single_stream_report,
    },
    benchmark_runner::{
        BenchmarkApplication, BenchmarkCommand, PairedBenchmarkPlan, PairedBenchmarkReport,
        run_paired_benchmark,
    },
};

const PAIRS: u32 = 10;

#[test]
fn non_range_single_stream_runs_ten_pairs_and_reports_reproducible_overhead() {
    let body = Arc::new(
        (0_u32..2 * 1024 * 1024)
            .map(|value| u8::try_from(value % 251).expect("byte pattern"))
            .collect::<Vec<_>>(),
    );
    let requests = Arc::new(Mutex::new(Vec::new()));
    let (source, server) = start_non_range_server(Arc::clone(&body), Arc::clone(&requests));
    let temporary = tempfile::tempdir().expect("temporary benchmark directory");
    let client = PathBuf::from(env!(
        "CARGO_BIN_EXE_download-manager-http1-benchmark-client"
    ));
    let application = |name: &str| BenchmarkApplication {
        name: name.to_owned(),
        version_command: BenchmarkCommand {
            program: client.clone(),
            arguments: vec!["--version".to_owned()],
        },
        download_command: BenchmarkCommand {
            program: client.clone(),
            arguments: vec![
                "download-single-stream".to_owned(),
                "{url}".to_owned(),
                "{output}".to_owned(),
            ],
        },
    };
    let plan = PairedBenchmarkPlan {
        schema_version: 1,
        scenario: NON_RANGE_SINGLE_STREAM_SCENARIO.to_owned(),
        protocol: "HTTP/1.1".to_owned(),
        seed: 0x51_65_01,
        pairs: PAIRS,
        source: format!("{source}?pair={{pair}}&seed={{seed}}"),
        output_directory: temporary.path().join("outputs"),
        retain_outputs: false,
        baseline: application("IDM fixture"),
        candidate: application("Download Manager fixture"),
    };

    let paired = run_paired_benchmark(&plan).expect("non-range paired benchmark");
    server.join().expect("non-range fixture server");
    let evidence =
        build_non_range_single_stream_report(&paired).expect("valid single-stream evidence");

    assert_eq!(evidence.paired_report.samples.len(), PAIRS as usize);
    assert_eq!(
        evidence.maximum_allowed_overhead_percent,
        MAXIMUM_SINGLE_STREAM_OVERHEAD_PERCENT
    );
    assert!(evidence.completion_time_overhead_percent.median.is_finite());
    assert!(paired.samples.iter().all(|sample| {
        sample.baseline.bytes == body.len() as u64
            && sample.candidate.bytes == body.len() as u64
            && sample.output_hash_matches
    }));

    let captured = requests.lock().expect("captured request lock");
    assert_eq!(captured.len(), (PAIRS * 2) as usize);
    let mut per_pair = BTreeMap::<String, usize>::new();
    for request in captured.iter() {
        assert_eq!(request.version, "HTTP/1.1");
        assert_eq!(request.range, None, "single-stream request sent Range");
        *per_pair.entry(request.target.clone()).or_default() += 1;
    }
    assert_eq!(per_pair.len(), PAIRS as usize);
    assert!(per_pair.values().all(|count| *count == 2));

    let report_json = serde_json::to_vec_pretty(&evidence).expect("JSON overhead report");
    let report_markdown = evidence.render_markdown();
    let json_path = temporary.path().join("non-range-single-stream.json");
    let markdown_path = temporary.path().join("non-range-single-stream.md");
    fs::write(&json_path, &report_json).expect("write JSON overhead report");
    fs::write(&markdown_path, &report_markdown).expect("write Markdown overhead report");
    let parsed = serde_json::from_slice::<serde_json::Value>(
        &fs::read(json_path).expect("read JSON overhead report"),
    )
    .expect("parse JSON overhead report");
    assert_eq!(
        parsed["paired_report"]["samples"].as_array().map(Vec::len),
        Some(PAIRS as usize)
    );
    assert_eq!(parsed["maximum_allowed_overhead_percent"], 3.0);
    assert!(report_markdown.contains("Maximum allowed median overhead: **3.00%**"));
    assert!(report_markdown.contains("All output hashes match: **true**"));
    assert!(report_markdown.contains("Overhead gate: **"));

    assert_three_percent_boundary(paired);
}

fn assert_three_percent_boundary(paired: PairedBenchmarkReport) {
    let mut at_limit = paired.clone();
    set_elapsed_times(&mut at_limit, 1_000_000, 1_030_000);
    let passing =
        build_non_range_single_stream_report(&at_limit).expect("exact-limit overhead report");
    assert!((passing.completion_time_overhead_percent.median - 3.0).abs() < 1e-9);
    assert!(passing.overhead_within_limit);

    let mut over_limit = paired;
    set_elapsed_times(&mut over_limit, 1_000_000, 1_030_001);
    let failing =
        build_non_range_single_stream_report(&over_limit).expect("over-limit overhead report");
    assert!(failing.completion_time_overhead_percent.median > 3.0);
    assert!(!failing.overhead_within_limit);
    assert!(
        failing
            .render_markdown()
            .contains("Overhead gate: **FAIL**")
    );
}

fn set_elapsed_times(report: &mut PairedBenchmarkReport, baseline: u64, candidate: u64) {
    for sample in &mut report.samples {
        sample.baseline.elapsed_micros = baseline;
        sample.candidate.elapsed_micros = candidate;
    }
}

#[derive(Debug)]
struct CapturedRequest {
    target: String,
    version: String,
    range: Option<String>,
}

fn start_non_range_server(
    body: Arc<Vec<u8>>,
    requests: Arc<Mutex<Vec<CapturedRequest>>>,
) -> (String, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind non-range fixture");
    let address = listener.local_addr().expect("fixture address");
    let server = thread::spawn(move || {
        for connection in listener.incoming().take((PAIRS * 2) as usize) {
            let mut stream = connection.expect("accept non-range request");
            let request = read_request(&mut stream);
            requests.lock().expect("request lock").push(request);
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nAccept-Ranges: none\r\nConnection: close\r\n\r\n",
                body.len(),
            )
            .expect("response headers");
            stream.write_all(&body).expect("response body");
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
    let mut lines = text.split("\r\n");
    let request_line = lines.next().expect("request line");
    let mut parts = request_line.split_whitespace();
    assert_eq!(parts.next(), Some("GET"));
    let target = parts.next().expect("request target").to_owned();
    let version = parts.next().expect("request version").to_owned();
    let range = lines.find_map(|line| {
        let (name, value) = line.split_once(':')?;
        name.eq_ignore_ascii_case("range")
            .then(|| value.trim().to_owned())
    });
    CapturedRequest {
        target,
        version,
        range,
    }
}
