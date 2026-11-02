use std::{
    fs,
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    path::{Path, PathBuf},
    thread,
    time::Duration,
};

use download_manager_engine::{
    dash::{DashAdaptation, parse_static_mpd},
    domain::HttpUrl,
    hls::{HlsLiveControl, HlsLiveGap, HlsLiveRecorder, HlsLiveStopCondition, HlsLiveStopReason},
    media::parse_hls_media_playlist,
    media_mux::{AudioCodec, MuxExpectation, MuxPlan, VideoCodec, validate_ffprobe_json},
    subtitle::{SubtitleFormat, convert_to_srt},
};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use url::Url;

const EXPECTED_LIVE_SHA256: &str =
    "f524fd50d821219cdb9e01cebadce2e4e769b87f8f5c4396b0c87d594fede80c";

#[derive(Debug, Deserialize)]
struct CorpusManifest {
    name: String,
    license_file: String,
    artifacts: Vec<CorpusArtifact>,
}

#[derive(Debug, Deserialize)]
struct CorpusArtifact {
    path: String,
    sha256: String,
    spdx: String,
}

#[test]
fn licensed_corpus_hashes_dash_subtitles_and_av_sync_are_stable() {
    let root = corpus_root();
    let manifest: CorpusManifest = serde_json::from_slice(
        &fs::read(root.join("corpus.json")).expect("read S174 corpus manifest"),
    )
    .expect("parse S174 corpus manifest");
    assert!(manifest.name.contains("S174"));
    let license = fs::read_to_string(root.join(&manifest.license_file)).expect("read CC0 license");
    assert!(license.contains("SPDX-License-Identifier: CC0-1.0"));
    assert_eq!(manifest.artifacts.len(), 11);
    for artifact in &manifest.artifacts {
        assert_eq!(artifact.spdx, "CC0-1.0", "{} license", artifact.path);
        assert_eq!(
            sha256(&fs::read(root.join(&artifact.path)).expect("read corpus artifact")),
            artifact.sha256,
            "{} hash",
            artifact.path
        );
    }

    let mpd = fs::read_to_string(root.join("static-av-subtitles.mpd")).expect("read MPD");
    let parsed = parse_static_mpd(
        &mpd,
        &Url::parse("https://media.example.test/cc0/manifest.mpd").expect("manifest URL"),
    )
    .expect("parse static corpus MPD");
    assert_eq!(parsed.periods.len(), 1);
    assert_eq!(parsed.periods[0].duration_millis, Some(6_000));
    assert_track_duration(adaptation(&parsed.periods[0].adaptations, "video"), 6);
    assert_track_duration(adaptation(&parsed.periods[0].adaptations, "audio"), 6);
    let captions = adaptation(&parsed.periods[0].adaptations, "text");
    assert_eq!(captions.language.as_deref(), Some("en"));
    assert_eq!(
        captions.representations[0].base_url.as_str(),
        "https://media.example.test/cc0/captions-en.vtt"
    );

    let vtt = convert_to_srt(
        &fs::read(root.join("captions-en.vtt")).expect("read WebVTT"),
        SubtitleFormat::WebVtt,
    )
    .expect("convert WebVTT");
    let ttml = convert_to_srt(
        &fs::read(root.join("captions-bn.ttml")).expect("read TTML"),
        SubtitleFormat::Ttml,
    )
    .expect("convert TTML");
    assert_eq!(vtt.cue_count, 3);
    assert_eq!(ttml.cue_count, 2);
    assert!(vtt.contents.contains("00:00:04,000 --> 00:00:06,000"));
    assert!(ttml.contents.contains("00:00:03,000 --> 00:00:06,000"));

    let expected = MuxExpectation::new(
        MuxPlan::select(VideoCodec::H264, AudioCodec::Aac),
        Duration::from_secs(6),
    )
    .expect("six-second mux expectation");
    let sync = validate_ffprobe_json(
        &fs::read(root.join("dash-ffprobe.json")).expect("read ffprobe evidence"),
        expected,
    )
    .expect("DASH A/V duration and synchronization");
    assert_eq!(sync.format_duration, Duration::from_secs(6));
    assert_eq!(sync.av_start_delta, Duration::from_millis(20));
    assert_eq!(sync.av_end_delta, Duration::ZERO);
}

#[test]
fn licensed_clear_live_corpus_deduplicates_gaps_and_stops_at_complete_duration() {
    let root = corpus_root();
    let (base_url, server) = serve_live_segments();
    let snapshots = [
        "live-window-early.m3u8",
        "live-window-overlap.m3u8",
        "live-window-late.m3u8",
    ]
    .map(|name| {
        parse_hls_media_playlist(
            &fs::read_to_string(root.join(name)).expect("read live window"),
            &HttpUrl::parse(format!("{base_url}/{name}")).expect("live manifest URL"),
        )
        .expect("parse clear live window")
    });
    let output = tempfile::tempdir()
        .expect("live output directory")
        .path()
        .join("recording.bin");
    let report = HlsLiveRecorder::new()
        .expect("live recorder")
        .record(
            snapshots,
            &output,
            HlsLiveStopCondition::Duration(Duration::from_secs(6)),
            &HlsLiveControl::default(),
        )
        .expect("record licensed clear-live corpus");
    server.join().expect("live fixture server");

    assert_eq!(report.stop_reason, HlsLiveStopReason::DurationReached);
    assert_eq!(report.recorded_duration, Duration::from_secs(6));
    assert_eq!(report.media_segments, 3);
    assert_eq!(report.downloaded_bytes, 63);
    assert_eq!(
        report.gaps,
        vec![HlsLiveGap {
            first_sequence: 42,
            last_sequence: 42,
        }]
    );
    assert_eq!(report.sha256, EXPECTED_LIVE_SHA256);
    assert_eq!(
        sha256(&fs::read(output).expect("read live output")),
        report.sha256
    );

    let expected = MuxExpectation::new(
        MuxPlan::select(VideoCodec::H264, AudioCodec::Aac),
        Duration::from_secs(6),
    )
    .expect("live mux expectation");
    let sync = validate_ffprobe_json(
        &fs::read(root.join("live-ffprobe.json")).expect("read live ffprobe evidence"),
        expected,
    )
    .expect("live duration and synchronization");
    assert_eq!(sync.format_duration, Duration::from_millis(6_020));
    assert_eq!(sync.av_start_delta, Duration::from_millis(20));
    assert_eq!(sync.av_end_delta, Duration::from_millis(20));
}

fn corpus_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("media_s174")
}

fn adaptation<'a>(adaptations: &'a [DashAdaptation], kind: &str) -> &'a DashAdaptation {
    adaptations
        .iter()
        .find(|adaptation| adaptation.content_type.as_deref() == Some(kind))
        .unwrap_or_else(|| panic!("missing {kind} adaptation"))
}

fn assert_track_duration(adaptation: &DashAdaptation, expected_units: u64) {
    let representation = adaptation.representations.first().expect("representation");
    let plan = representation.segment_plan.as_ref().expect("segment plan");
    assert!(plan.initialization.is_some());
    assert_eq!(plan.media.len(), 3);
    assert_eq!(
        plan.media
            .iter()
            .map(|segment| segment.duration)
            .sum::<u64>(),
        expected_units
    );
}

fn sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn serve_live_segments() -> (String, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind live corpus server");
    let address = listener.local_addr().expect("live corpus address");
    let server = thread::spawn(move || {
        for _ in 0..3 {
            let (mut stream, _) = listener.accept().expect("accept live segment request");
            respond_with_segment(&mut stream);
        }
    });
    (format!("http://{address}"), server)
}

fn respond_with_segment(stream: &mut TcpStream) {
    let mut request = [0_u8; 2_048];
    let received = stream.read(&mut request).expect("read live request");
    let request = String::from_utf8_lossy(&request[..received]);
    let path = request
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .expect("request path");
    let (status, body): (&str, &[u8]) = match path {
        "/segment-040.bin" => (
            "200 OK",
            include_bytes!("fixtures/media_s174/segment-040.bin"),
        ),
        "/segment-041.bin" => (
            "200 OK",
            include_bytes!("fixtures/media_s174/segment-041.bin"),
        ),
        "/segment-043.bin" => (
            "200 OK",
            include_bytes!("fixtures/media_s174/segment-043.bin"),
        ),
        _ => ("404 Not Found", b"missing"),
    };
    write!(
        stream,
        "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    )
    .expect("write live response headers");
    stream.write_all(body).expect("write live response body");
}
