use std::collections::HashSet;

use download_manager_engine::{
    domain::HttpUrl,
    grabber::{HtmlCrawlError, SameHostHtmlCrawler, canonicalize_http_reference},
    grabber_css::{CssDiscoveryError, discover_css_assets},
    grabber_offline::{
        OfflineMapper, OfflineRewriteError, rewrite_css_offline, rewrite_html_offline,
    },
};
use proptest::prelude::*;
use url::Url;

const HTML_LIMIT: usize = 8 * 1024 * 1024;
const CSS_DISCOVERY_LIMIT: usize = 1024 * 1024;
const MAX_HTML_LINKS: usize = 8_192;
const MAX_CSS_URLS: usize = 4_096;

fn document_bytes(limit: usize) -> impl Strategy<Value = Vec<u8>> {
    prop_oneof![
        31 => prop::collection::vec(any::<u8>(), 0..=32_768),
        1 => prop::collection::vec(any::<u8>(), (limit - 16)..=(limit + 16)),
    ]
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

fn root_url() -> HttpUrl {
    HttpUrl::parse("https://example.test/root/index.html").expect("static root URL")
}

proptest! {
    #[test]
    fn crawler_url_normalization_is_idempotent_and_strips_unsafe_parts(reference in any::<String>()) {
        let base = root_url();
        let Some(normalized) = canonicalize_http_reference(&base, &reference) else {
            return Ok(());
        };
        let parsed = Url::parse(normalized.as_str()).expect("crawler returned a parsed URL");

        prop_assert!(matches!(parsed.scheme(), "http" | "https"));
        prop_assert!(parsed.host().is_some());
        prop_assert!(parsed.username().is_empty());
        prop_assert!(parsed.password().is_none());
        prop_assert!(parsed.fragment().is_none());
        prop_assert_eq!(
            canonicalize_http_reference(&base, normalized.as_str()),
            Some(normalized),
        );
    }

    #[test]
    fn html_crawler_and_offline_rewriter_handle_bounded_arbitrary_documents(
        bytes in document_bytes(HTML_LIMIT),
        reference in any::<String>(),
    ) {
        let root = root_url();
        let body = text(&bytes);
        exercise_html_document(&root, &body)?;

        if reference.len() <= 32_768 {
            let wrapped = format!(
                "<base href=\"{reference}\"><a href=\"{reference}\">fuzz</a>"
            );
            exercise_html_document(&root, &wrapped)?;
        }
    }

    #[test]
    fn css_discovery_and_offline_rewriter_handle_bounded_arbitrary_documents(
        bytes in document_bytes(CSS_DISCOVERY_LIMIT),
        reference in any::<String>(),
    ) {
        let root = root_url();
        let stylesheet_url = Url::parse("https://example.test/css/site.css")
            .expect("static stylesheet URL");
        let body = text(&bytes);
        exercise_css_document(&root, &stylesheet_url, &body)?;

        if reference.len() <= 32_768 {
            let wrapped = format!(
                "@import \"{reference}\"; .fuzz {{ background: url('{reference}') }}"
            );
            exercise_css_document(&root, &stylesheet_url, &wrapped)?;
        }
    }
}

fn exercise_html_document(root: &HttpUrl, document: &str) -> Result<(), TestCaseError> {
    let mut crawler = SameHostHtmlCrawler::new(root).expect("static crawler root");
    let request = crawler
        .next_request()
        .expect("root request is within the safety page cap")
        .expect("root request exists");
    let submitted = crawler.submit_html(request, document);
    if document.len() > HTML_LIMIT {
        prop_assert!(
            matches!(submitted, Err(HtmlCrawlError::HtmlTooLarge { .. })),
            "oversized HTML should be rejected"
        );
    } else {
        match submitted {
            Ok(()) => {
                let mut links = Vec::new();
                while let Some(request) = crawler.next_request().expect("bounded frontier") {
                    links.push(request.url);
                }
                prop_assert!(links.len() <= MAX_HTML_LINKS);
                let mut unique = HashSet::<String>::new();
                for link in links {
                    let parsed = Url::parse(link.as_str()).expect("crawler emitted parsed URL");
                    prop_assert_eq!(parsed.host_str(), Some("example.test"));
                    prop_assert!(parsed.fragment().is_none());
                    prop_assert!(unique.insert(link.as_str().to_owned()));
                }
            }
            Err(HtmlCrawlError::TooManyLinks { .. }) => {}
            Err(error) => prop_assert!(false, "unexpected HTML crawler error: {error}"),
        }
    }

    let mapper = OfflineMapper::new(std::env::temp_dir().join("download-manager-s161"), root)
        .expect("absolute fuzz destination");
    let rewritten = rewrite_html_offline(&mapper, root, document);
    if document.len() > HTML_LIMIT {
        prop_assert_eq!(rewritten, Err(OfflineRewriteError::DocumentTooLarge));
    } else {
        prop_assert!(rewritten.is_ok());
    }
    Ok(())
}

fn exercise_css_document(
    root: &HttpUrl,
    stylesheet_url: &Url,
    document: &str,
) -> Result<(), TestCaseError> {
    let discovered = discover_css_assets(stylesheet_url, document);
    if document.len() > CSS_DISCOVERY_LIMIT {
        prop_assert_eq!(discovered, Err(CssDiscoveryError::StylesheetTooLarge));
    } else {
        match discovered {
            Ok(urls) => {
                prop_assert!(urls.len() <= MAX_CSS_URLS);
                let mut unique = HashSet::new();
                for url in urls {
                    prop_assert_eq!(url.host_str(), Some("example.test"));
                    prop_assert!(url.fragment().is_none());
                    prop_assert!(unique.insert(url.as_str().to_owned()));
                }
            }
            Err(CssDiscoveryError::TooManyUrls) => {}
            Err(error) => prop_assert!(false, "unexpected CSS discovery error: {error}"),
        }
    }

    let mapper = OfflineMapper::new(std::env::temp_dir().join("download-manager-s161"), root)
        .expect("absolute fuzz destination");
    let stylesheet = HttpUrl::parse(stylesheet_url.as_str()).expect("static stylesheet URL");
    prop_assert!(rewrite_css_offline(&mapper, &stylesheet, document).is_ok());
    Ok(())
}
