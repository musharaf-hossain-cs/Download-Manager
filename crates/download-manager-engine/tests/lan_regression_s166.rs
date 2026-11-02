use std::{
    fs,
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    path::PathBuf,
    sync::{Arc, Mutex},
    thread,
};

use download_manager_engine::{
    benchmark_lan::{
        LAN_SYNC_REGRESSION_SCENARIO, PROFILE_COPY_CHUNK_BYTES, build_lan_regression_report,
    },
    benchmark_runner::{
        BenchmarkApplication, BenchmarkCommand, PairedBenchmarkPlan, run_paired_benchmark,
    },
};

const PAIRS: u32 = 10;
const BODY_BYTES: usize = 2 * 1024 * 1024;

#[test]
fn per_chunk_sync_regression_is_measured_fixed_and_correct() {
    let body = Arc::new(
        (0..BODY_BYTES)
            .map(|value| u8::try_from(value % 251).expect("byte pattern"))
            .collect::<Vec<_>>(),
    );
    let requests = Arc::new(Mutex::new(Vec::new()));
    let (source, server) = start_server(Arc::clone(&body), Arc::clone(&requests));
    let temporary = tempfile::tempdir().expect("temporary benchmark directory");
    let client = PathBuf::from(env!(
        "CARGO_BIN_EXE_download-manager-http1-benchmark-client"
    ));
    let application = |name: &str, command: &str| BenchmarkApplication {
        name: name.to_owned(),
        version_command: BenchmarkCommand {
            program: client.clone(),
            arguments: vec!["--version".to_owned()],
        },
        download_command: BenchmarkCommand {
            program: client.clone(),
            arguments: vec![
                command.to_owned(),
                "{url}".to_owned(),
                "{output}".to_owned(),
            ],
        },
    };
    let plan = PairedBenchmarkPlan {
        schema_version: 1,
        scenario: LAN_SYNC_REGRESSION_SCENARIO.to_owned(),
        protocol: "HTTP/1.1".to_owned(),
        seed: 0x51_66_01,
        pairs: PAIRS,
        source: format!("{source}?pair={{pair}}&seed={{seed}}"),
        output_directory: temporary.path().join("outputs"),
        retain_outputs: false,
        baseline: application("before: sync each chunk", "profile-before"),
        candidate: application("after: sync on completion", "profile-after"),
    };

    let paired = run_paired_benchmark(&plan).expect("paired regression benchmark");
    server.join().expect("LAN fixture server");
    let evidence = build_lan_regression_report(&paired).expect("improved LAN evidence");

    assert_eq!(evidence.before_sync_calls_per_download, 32);
    assert_eq!(PROFILE_COPY_CHUNK_BYTES, 64 * 1024);
    assert_eq!(evidence.after_sync_calls_per_download, 1);
    assert!(evidence.median_goodput_improvement_percent > 0.0);
    assert!(evidence.correctness_preserved);
    assert!(paired.samples.iter().all(|sample| {
        sample.baseline.bytes == BODY_BYTES as u64
            && sample.candidate.bytes == BODY_BYTES as u64
            && sample.output_hash_matches
    }));

    let captured = requests.lock().expect("captured request lock");
    assert_eq!(captured.len(), (PAIRS * 2) as usize);
    assert!(captured.iter().all(|request| request.range.is_none()));

    let json = serde_json::to_vec_pretty(&evidence).expect("JSON profile report");
    let markdown = evidence.render_markdown();
    let json_path = temporary.path().join("lan-regression.json");
    let markdown_path = temporary.path().join("lan-regression.md");
    fs::write(&json_path, json).expect("write JSON profile report");
    fs::write(&markdown_path, &markdown).expect("write Markdown profile report");
    let parsed: serde_json::Value =
        serde_json::from_slice(&fs::read(json_path).expect("read JSON profile report"))
            .expect("parse JSON profile report");
    assert_eq!(parsed["before_sync_calls_per_download"], 32);
    assert!(markdown.contains("Bottleneck: **durability sync in every 64 KiB"));
    assert!(markdown.contains("Correctness preserved: **true**"));
}

#[derive(Debug)]
struct CapturedRequest {
    range: Option<String>,
}

fn start_server(
    body: Arc<Vec<u8>>,
    requests: Arc<Mutex<Vec<CapturedRequest>>>,
) -> (String, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind LAN fixture");
    let address = listener.local_addr().expect("fixture address");
    let server = thread::spawn(move || {
        for connection in listener.incoming().take((PAIRS * 2) as usize) {
            let mut stream = connection.expect("accept LAN request");
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
    let range = text.split("\r\n").find_map(|line| {
        let (name, value) = line.split_once(':')?;
        name.eq_ignore_ascii_case("range")
            .then(|| value.trim().to_owned())
    });
    CapturedRequest { range }
}
