use download_manager_engine::{
    dash::parse_static_mpd,
    domain::HttpUrl,
    media::{parse_hls_master_playlist, parse_hls_media_playlist},
};
use proptest::prelude::*;
use url::Url;

const MANIFEST_LIMIT: usize = 1_048_576;

fn manifest_bytes() -> impl Strategy<Value = Vec<u8>> {
    prop_oneof![
        31 => prop::collection::vec(any::<u8>(), 0..=32_768),
        1 => prop::collection::vec(
            any::<u8>(),
            (MANIFEST_LIMIT - 16)..=(MANIFEST_LIMIT + 16),
        ),
    ]
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

proptest! {
    #[test]
    fn hls_parsers_handle_arbitrary_bounded_documents(bytes in manifest_bytes()) {
        let document = text(&bytes);
        let master_url = HttpUrl::parse("https://media.example.test/master/index.m3u8")
            .expect("static master URL");
        let media_url = HttpUrl::parse("https://media.example.test/video/index.m3u8")
            .expect("static media URL");

        let _ = parse_hls_master_playlist(&document, &master_url);
        let _ = parse_hls_media_playlist(&document, &media_url);

        if document.len() <= MANIFEST_LIMIT {
            let with_header = format!("#EXTM3U\n{document}");
            let _ = parse_hls_master_playlist(&with_header, &master_url);
            let _ = parse_hls_media_playlist(&with_header, &media_url);
        }
    }

    #[test]
    fn dash_parser_handles_arbitrary_bounded_documents(bytes in manifest_bytes()) {
        let document = text(&bytes);
        let manifest_url = Url::parse("https://media.example.test/root/manifest.mpd")
            .expect("static DASH URL");

        let _ = parse_static_mpd(&document, &manifest_url);
        if document.len() <= MANIFEST_LIMIT.saturating_sub(32) {
            let wrapped = format!("<MPD type=\"static\">{document}</MPD>");
            let _ = parse_static_mpd(&wrapped, &manifest_url);
        }
    }
}
