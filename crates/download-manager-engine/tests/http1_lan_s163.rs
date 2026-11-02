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
    benchmark_lan::validate_http1_lan_report,
    benchmark_runner::{
        BenchmarkApplication, BenchmarkCommand, PairedBenchmarkPlan, run_paired_benchmark,
    },
};

const PAIRS: u32 = 10;

#[test]
fn controlled_http1_lan_runs_ten_pairs_with_hashes_and_reports() {
    let body = Arc::new(
        (0_u32..512 * 1024)
            .map(|value| u8::try_from(value % 251).expect("byte pattern"))
            .collect::<Vec<_>>(),
    );
    let requests = Arc::new(Mutex::new(Vec::new()));
    let (source, server) = start_http1_server(Arc::clone(&body), Arc::clone(&requests));
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
                "download".to_owned(),
                "{url}".to_owned(),
                "{output}".to_owned(),
            ],
        },
    };
    let plan = PairedBenchmarkPlan {
        schema_version: 1,
        scenario: "controlled-http1-lan-fixture".to_owned(),
        protocol: "HTTP/1.1".to_owned(),
        seed: 0x51_63_11,
        pairs: PAIRS,
        source: format!("{source}?pair={{pair}}&seed={{seed}}"),
        output_directory: temporary.path().join("outputs"),
        retain_outputs: false,
        baseline: application("IDM fixture"),
        candidate: application("Download Manager fixture"),
    };

    let report = run_paired_benchmark(&plan).expect("controlled HTTP/1.1 benchmark");
    server.join().expect("HTTP/1.1 server");
    validate_http1_lan_report(&report).expect("valid controlled LAN evidence");

    assert_eq!(report.samples.len(), PAIRS as usize);
    assert!(report.samples.iter().all(|sample| {
        sample.baseline.bytes == body.len() as u64
            && sample.candidate.bytes == body.len() as u64
            && sample.output_hash_matches
    }));
    let captured = requests.lock().expect("captured request lock");
    assert_eq!(captured.len(), (PAIRS * 2) as usize);
    let mut per_pair = BTreeMap::<String, usize>::new();
    for request in captured.iter() {
        assert_eq!(request.version, "HTTP/1.1");
        assert_eq!(request.range.as_deref(), Some("bytes=0-"));
        *per_pair.entry(request.target.clone()).or_default() += 1;
    }
    assert_eq!(per_pair.len(), PAIRS as usize);
    assert!(per_pair.values().all(|count| *count == 2));

    let report_json = serde_json::to_vec_pretty(&report).expect("JSON report");
    let report_markdown = report.render_markdown();
    let json_path = temporary.path().join("http1-lan.json");
    let markdown_path = temporary.path().join("http1-lan.md");
    fs::write(&json_path, &report_json).expect("write JSON report");
    fs::write(&markdown_path, &report_markdown).expect("write Markdown report");
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&fs::read(json_path).expect("read JSON"))
            .expect("parse JSON")["samples"]
            .as_array()
            .expect("sample array")
            .len(),
        PAIRS as usize
    );
    assert!(report_markdown.contains("Protocol: `HTTP/1.1`"));
    assert!(report_markdown.contains("pairs: `10`"));
    assert!(report_markdown.contains("All output hashes match: **true**"));
}

#[derive(Debug)]
struct CapturedRequest {
    target: String,
    version: String,
    range: Option<String>,
}

fn start_http1_server(
    body: Arc<Vec<u8>>,
    requests: Arc<Mutex<Vec<CapturedRequest>>>,
) -> (String, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind HTTP/1.1 fixture");
    let address = listener.local_addr().expect("fixture address");
    let server = thread::spawn(move || {
        for connection in listener.incoming().take((PAIRS * 2) as usize) {
            let mut stream = connection.expect("accept HTTP/1.1 request");
            let request = read_request(&mut stream);
            requests.lock().expect("request lock").push(request);
            write!(
                stream,
                "HTTP/1.1 206 Partial Content\r\nContent-Length: {}\r\nContent-Range: bytes 0-{}/{}\r\nConnection: close\r\n\r\n",
                body.len(),
                body.len() - 1,
                body.len()
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
