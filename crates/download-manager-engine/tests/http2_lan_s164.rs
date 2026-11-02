use std::{
    collections::BTreeMap,
    fs,
    net::TcpListener,
    path::PathBuf,
    sync::{Arc, Mutex},
    thread,
};

use bytes::Bytes;
use download_manager_engine::{
    benchmark_lan::validate_http2_lan_report,
    benchmark_runner::{
        BenchmarkApplication, BenchmarkCommand, PairedBenchmarkPlan, run_paired_benchmark,
    },
};
use http::{Method, Request, StatusCode, Version, header};

const PAIRS: u32 = 10;

#[test]
fn controlled_http2_lan_runs_ten_pairs_with_hashes_and_reports() {
    let body = Arc::new(
        (0_u32..512 * 1024)
            .map(|value| u8::try_from(value % 251).expect("byte pattern"))
            .collect::<Vec<_>>(),
    );
    let requests = Arc::new(Mutex::new(Vec::new()));
    let (source, server) = start_http2_server(Arc::clone(&body), Arc::clone(&requests));
    let temporary = tempfile::tempdir().expect("temporary benchmark directory");
    let client = PathBuf::from(env!(
        "CARGO_BIN_EXE_download-manager-http2-benchmark-client"
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
        scenario: "controlled-http2-lan-fixture".to_owned(),
        protocol: "HTTP/2".to_owned(),
        seed: 0x51_64_22,
        pairs: PAIRS,
        source: format!("{source}?pair={{pair}}&seed={{seed}}"),
        output_directory: temporary.path().join("outputs"),
        retain_outputs: false,
        baseline: application("IDM fixture"),
        candidate: application("Download Manager fixture"),
    };

    let report = run_paired_benchmark(&plan).expect("controlled HTTP/2 benchmark");
    server.join().expect("HTTP/2 server");
    validate_http2_lan_report(&report).expect("valid controlled HTTP/2 LAN evidence");

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
        assert_eq!(request.version, Version::HTTP_2);
        assert_eq!(request.range.as_deref(), Some("bytes=0-"));
        *per_pair.entry(request.target.clone()).or_default() += 1;
    }
    assert_eq!(per_pair.len(), PAIRS as usize);
    assert!(per_pair.values().all(|count| *count == 2));

    let report_json = serde_json::to_vec_pretty(&report).expect("JSON report");
    let report_markdown = report.render_markdown();
    let json_path = temporary.path().join("http2-lan.json");
    let markdown_path = temporary.path().join("http2-lan.md");
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
    assert!(report_markdown.contains("Protocol: `HTTP/2`"));
    assert!(report_markdown.contains("pairs: `10`"));
    assert!(report_markdown.contains("All output hashes match: **true**"));
}

#[derive(Debug)]
struct CapturedRequest {
    target: String,
    version: Version,
    range: Option<String>,
}

fn start_http2_server(
    body: Arc<Vec<u8>>,
    requests: Arc<Mutex<Vec<CapturedRequest>>>,
) -> (String, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind HTTP/2 fixture");
    listener
        .set_nonblocking(true)
        .expect("make HTTP/2 fixture nonblocking");
    let address = listener.local_addr().expect("fixture address");
    let server = thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_io()
            .build()
            .expect("HTTP/2 fixture runtime");
        runtime.block_on(async move {
            let listener = tokio::net::TcpListener::from_std(listener)
                .expect("convert HTTP/2 fixture listener");
            for _ in 0..(PAIRS * 2) {
                let (stream, _) = listener.accept().await.expect("accept HTTP/2 request");
                serve_connection(stream, Arc::clone(&body), Arc::clone(&requests)).await;
            }
        });
    });
    (format!("http://{address}/fixture.bin"), server)
}

async fn serve_connection(
    stream: tokio::net::TcpStream,
    body: Arc<Vec<u8>>,
    requests: Arc<Mutex<Vec<CapturedRequest>>>,
) {
    let mut connection = h2::server::handshake(stream)
        .await
        .expect("HTTP/2 handshake");
    let Some(request) = connection.accept().await else {
        panic!("HTTP/2 client closed before sending a request");
    };
    let (request, mut response) = request.expect("valid HTTP/2 request");
    capture_request(&request, &requests);
    let reply = http::Response::builder()
        .status(StatusCode::PARTIAL_CONTENT)
        .header(header::CONTENT_LENGTH, body.len())
        .header(
            header::CONTENT_RANGE,
            format!("bytes 0-{}/{}", body.len() - 1, body.len()),
        )
        .body(())
        .expect("HTTP/2 response");
    let mut send = response
        .send_response(reply, false)
        .expect("send HTTP/2 headers");
    send.send_data(Bytes::copy_from_slice(&body), true)
        .expect("send HTTP/2 body");
    if let Some(request) = connection.accept().await {
        panic!("unexpected extra HTTP/2 request: {request:?}");
    }
}

fn capture_request(request: &Request<h2::RecvStream>, requests: &Mutex<Vec<CapturedRequest>>) {
    assert_eq!(request.method(), Method::GET);
    let range = request
        .headers()
        .get(header::RANGE)
        .map(|value| value.to_str().expect("ASCII range header").to_owned());
    requests
        .lock()
        .expect("request lock")
        .push(CapturedRequest {
            target: request
                .uri()
                .path_and_query()
                .expect("request path and query")
                .as_str()
                .to_owned(),
            version: request.version(),
            range,
        });
}
