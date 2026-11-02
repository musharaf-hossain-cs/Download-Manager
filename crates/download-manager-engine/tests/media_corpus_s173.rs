use std::{
    collections::BTreeMap,
    fs,
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    sync::Arc,
    thread,
    time::Duration,
};

use download_manager_engine::{
    domain::HttpUrl,
    hls::HlsVodDownloader,
    media::{parse_hls_master_playlist, parse_hls_media_playlist},
    media_mux::{AudioCodec, MuxExpectation, MuxPlan, VideoCodec, validate_ffprobe_json},
    media_worker::FfmpegContainer,
    transfer::SingleStreamDownloader,
};

const LICENSE: &str = include_str!("fixtures/s173/LICENSE.md");
const PROGRESSIVE: &[u8] = include_bytes!("fixtures/s173/progressive.mp4.fixture");
const TS_0: &[u8] = include_bytes!("fixtures/s173/hls-ts-000.ts.fixture");
const TS_1: &[u8] = include_bytes!("fixtures/s173/hls-ts-001.ts.fixture");
const FMP4_INIT: &[u8] = include_bytes!("fixtures/s173/hls-fmp4-init.mp4.fixture");
const FMP4_0: &[u8] = include_bytes!("fixtures/s173/hls-fmp4-000.m4s.fixture");
const FMP4_1: &[u8] = include_bytes!("fixtures/s173/hls-fmp4-001.m4s.fixture");

const PROGRESSIVE_SHA256: &str = "7d58d487c58cfa0bddb228ecc68068a807d6f7ab673caeba45dbfc94804a895e";
const HLS_TS_SHA256: &str = "4ac5acf592ce5d6a8736664800abb45e30275aefc9dbb1830acfad2bb57a9a18";
const HLS_FMP4_SHA256: &str = "dd03f79b79834c10f66ddde1dc137dd57947a039777f8a30dc2b8beea1085ddf";

#[test]
fn licensed_progressive_and_hls_corpus_has_stable_hashes_and_probe_results() {
    assert!(LICENSE.contains("Creative Commons CC0 1.0"));
    assert!(LICENSE.contains("no third-party"));

    let resources = BTreeMap::from([
        ("/progressive.mp4", PROGRESSIVE),
        ("/ts/000.ts", TS_0),
        ("/ts/001.ts", TS_1),
        ("/fmp4/init.mp4", FMP4_INIT),
        ("/fmp4/000.m4s", FMP4_0),
        ("/fmp4/001.m4s", FMP4_1),
    ]);
    let (origin, server) = start_fixture_server(resources);
    let temporary = tempfile::tempdir().expect("S173 output directory");

    let progressive_path = temporary.path().join("progressive.mp4");
    let progressive_url =
        HttpUrl::parse(format!("{origin}/progressive.mp4")).expect("progressive fixture URL");
    let progressive = SingleStreamDownloader::new()
        .expect("progressive downloader")
        .download(&progressive_url, &progressive_path, |_| {})
        .expect("progressive corpus transfer");
    assert_eq!(progressive.downloaded_bytes, PROGRESSIVE.len() as u64);
    assert_eq!(progressive.sha256, PROGRESSIVE_SHA256);
    assert_eq!(fs::read(&progressive_path).unwrap(), PROGRESSIVE);

    let master_url = HttpUrl::parse(format!("{origin}/master.m3u8")).expect("master URL");
    let master = parse_hls_master_playlist(
        "#EXTM3U\n\
         #EXT-X-STREAM-INF:BANDWIDTH=900000,RESOLUTION=640x360,CODECS=\"avc1.4d401e,mp4a.40.2\"\n\
         ts/index.m3u8\n\
         #EXT-X-STREAM-INF:BANDWIDTH=1800000,RESOLUTION=1280x720,CODECS=\"avc1.64001f,mp4a.40.2\"\n\
         fmp4/index.m3u8\n",
        &master_url,
    )
    .expect("multivariant HLS corpus");
    assert_eq!(master.variants.len(), 2);
    assert_eq!(master.variants[0].resolution, Some((640, 360)));
    assert_eq!(master.variants[1].resolution, Some((1280, 720)));

    let ts_playlist = parse_hls_media_playlist(
        "#EXTM3U\n\
         #EXT-X-VERSION:3\n\
         #EXT-X-TARGETDURATION:2\n\
         #EXTINF:2.000,\n\
         000.ts\n\
         #EXTINF:2.000,\n\
         001.ts\n\
         #EXT-X-ENDLIST\n",
        &master.variants[0].uri,
    )
    .expect("TS media playlist");
    let ts_path = temporary.path().join("hls-ts.bin");
    let ts = HlsVodDownloader::new()
        .expect("HLS downloader")
        .download(&ts_playlist, &ts_path)
        .expect("TS HLS corpus transfer");
    assert_eq!(ts.media_segments, 2);
    assert_eq!(ts.init_segments, 0);
    assert_eq!(ts.sha256, HLS_TS_SHA256);
    assert_eq!(fs::read(&ts_path).unwrap(), [TS_0, TS_1].concat());

    let fmp4_playlist = parse_hls_media_playlist(
        "#EXTM3U\n\
         #EXT-X-VERSION:7\n\
         #EXT-X-TARGETDURATION:2\n\
         #EXT-X-MAP:URI=\"init.mp4\"\n\
         #EXTINF:2.000,\n\
         000.m4s\n\
         #EXTINF:2.000,\n\
         001.m4s\n\
         #EXT-X-ENDLIST\n",
        &master.variants[1].uri,
    )
    .expect("fMP4 media playlist");
    let fmp4_path = temporary.path().join("hls-fmp4.bin");
    let fmp4 = HlsVodDownloader::new()
        .expect("HLS downloader")
        .download(&fmp4_playlist, &fmp4_path)
        .expect("fMP4 HLS corpus transfer");
    assert_eq!(fmp4.media_segments, 2);
    assert_eq!(fmp4.init_segments, 1);
    assert_eq!(fmp4.sha256, HLS_FMP4_SHA256);
    assert_eq!(
        fs::read(&fmp4_path).unwrap(),
        [FMP4_INIT, FMP4_0, FMP4_1].concat()
    );

    server.join().expect("fixture server");
    assert_probe(include_bytes!("fixtures/s173/progressive.ffprobe.json"));
    assert_probe(include_bytes!("fixtures/s173/hls-ts.ffprobe.json"));
    assert_probe(include_bytes!("fixtures/s173/hls-fmp4.ffprobe.json"));
}

fn assert_probe(report: &[u8]) {
    let expectation = MuxExpectation::new(
        MuxPlan::for_container(FfmpegContainer::Mp4, VideoCodec::H264, AudioCodec::Aac)
            .expect("MP4 codec plan"),
        Duration::from_secs(4),
    )
    .expect("probe expectation");
    let validated = validate_ffprobe_json(report, expectation).expect("valid ffprobe report");
    assert_eq!(validated.format_duration, Duration::from_secs(4));
    assert!(validated.av_start_delta <= Duration::from_millis(21));
    assert_eq!(validated.av_end_delta, Duration::ZERO);
}

fn start_fixture_server(
    resources: BTreeMap<&'static str, &'static [u8]>,
) -> (String, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind S173 fixture server");
    let address = listener.local_addr().expect("fixture address");
    let resources = Arc::new(resources);
    let request_count = resources.len();
    let server = thread::spawn(move || {
        for connection in listener.incoming().take(request_count) {
            let mut stream = connection.expect("accept fixture request");
            let path = read_request_path(&mut stream);
            let body = resources.get(path.as_str()).expect("known fixture path");
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nContent-Type: application/octet-stream\r\nConnection: close\r\n\r\n",
                body.len()
            )
            .expect("fixture response headers");
            stream.write_all(body).expect("fixture response body");
        }
    });
    (format!("http://{address}"), server)
}

fn read_request_path(stream: &mut TcpStream) -> String {
    let mut request = Vec::new();
    let mut buffer = [0_u8; 1024];
    while !request.windows(4).any(|window| window == b"\r\n\r\n") {
        let read = stream.read(&mut buffer).expect("request bytes");
        assert!(read > 0, "request closed before headers");
        request.extend_from_slice(&buffer[..read]);
        assert!(request.len() <= 16 * 1024, "bounded fixture request");
    }
    let request = String::from_utf8(request).expect("ASCII fixture request");
    request
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .expect("request path")
        .to_owned()
}
