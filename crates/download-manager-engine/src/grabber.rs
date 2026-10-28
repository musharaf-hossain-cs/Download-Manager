//! Static, transport-independent site crawling primitives.

use std::{
    collections::{HashMap, HashSet, VecDeque},
    fmt::Display,
};

use scraper::{Html, Selector};
use thiserror::Error;
use url::Url;

use crate::domain::HttpUrl;

const SAFETY_PAGE_LIMIT: usize = 10_000;
const MAX_HTML_BYTES_PER_PAGE: usize = 8 * 1024 * 1024;
const MAX_LINKS_PER_PAGE: usize = 8_192;

/// One pending HTML request emitted by the crawler.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HtmlCrawlRequest {
    pub url: HttpUrl,
    pub depth: u32,
}

/// One HTML page accepted during a same-host crawl.
pub type CrawledHtmlPage = HtmlCrawlRequest;

/// Ordered breadth-first result of a completed HTML crawl.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HtmlCrawlReport {
    pub pages: Vec<CrawledHtmlPage>,
}

/// A failure that prevents an HTML crawl from completing safely.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum HtmlCrawlError {
    #[error("start URL could not be normalized")]
    InvalidStartUrl,
    #[error("failed to fetch {url}: {message}")]
    Fetch { url: HttpUrl, message: String },
    #[error("HTML page exceeds the parser safety limit: {url}")]
    HtmlTooLarge { url: HttpUrl },
    #[error("HTML page contains too many crawlable links: {url}")]
    TooManyLinks { url: HttpUrl },
    #[error("page was not pending in this crawl: {url}")]
    UnexpectedPage { url: HttpUrl },
    #[error("crawl still has pending pages")]
    PendingPages,
    #[error("crawl exceeded the engine safety limit of {SAFETY_PAGE_LIMIT} pages")]
    SafetyPageLimit,
}

/// Incremental, transport-independent same-host HTML crawl state.
///
/// Callers may take more than one request before submitting a response, which lets later policy
/// layers impose concurrency, duration, byte, page, and depth limits without changing URL
/// discovery. Every canonical URL is issued at most once.
#[derive(Debug)]
pub struct SameHostHtmlCrawler {
    crawl_host: String,
    queued: HashSet<String>,
    frontier: VecDeque<HtmlCrawlRequest>,
    in_flight: HashMap<String, u32>,
    pages: Vec<CrawledHtmlPage>,
}

impl SameHostHtmlCrawler {
    /// Creates crawl state rooted at a canonical HTTP(S) URL.
    ///
    /// # Errors
    /// Returns [`HtmlCrawlError::InvalidStartUrl`] if the validated URL cannot be normalized.
    pub fn new(start_url: &HttpUrl) -> Result<Self, HtmlCrawlError> {
        let start = canonicalize_http_reference(start_url, start_url.as_str())
            .ok_or(HtmlCrawlError::InvalidStartUrl)?;
        let parsed = Url::parse(start.as_str()).map_err(|_| HtmlCrawlError::InvalidStartUrl)?;
        let crawl_host = parsed
            .host_str()
            .ok_or(HtmlCrawlError::InvalidStartUrl)?
            .to_owned();
        Ok(Self {
            crawl_host,
            queued: HashSet::from([start.as_str().to_owned()]),
            frontier: VecDeque::from([HtmlCrawlRequest {
                url: start,
                depth: 0,
            }]),
            in_flight: HashMap::new(),
            pages: Vec::new(),
        })
    }

    /// Takes the next breadth-first request, if the frontier is non-empty.
    ///
    /// # Errors
    /// Returns [`HtmlCrawlError::SafetyPageLimit`] before issuing more than the engine hard ceiling.
    pub fn next_request(&mut self) -> Result<Option<HtmlCrawlRequest>, HtmlCrawlError> {
        let Some(request) = self.frontier.pop_front() else {
            return Ok(None);
        };
        if self.pages.len() + self.in_flight.len() == SAFETY_PAGE_LIMIT {
            self.frontier.push_front(request);
            return Err(HtmlCrawlError::SafetyPageLimit);
        }
        self.in_flight
            .insert(request.url.as_str().to_owned(), request.depth);
        Ok(Some(request))
    }

    /// Accepts one fetched HTML body and discovers additional same-host links.
    ///
    /// # Errors
    /// Rejects responses that were not issued by this crawler and pages exceeding parser ceilings.
    pub fn submit_html(
        &mut self,
        request: HtmlCrawlRequest,
        html: &str,
    ) -> Result<(), HtmlCrawlError> {
        if self.in_flight.get(request.url.as_str()) != Some(&request.depth) {
            return Err(HtmlCrawlError::UnexpectedPage { url: request.url });
        }
        if html.len() > MAX_HTML_BYTES_PER_PAGE {
            return Err(HtmlCrawlError::HtmlTooLarge { url: request.url });
        }

        let links = discover_html_links(&request.url, html, &self.crawl_host)?;
        self.in_flight.remove(request.url.as_str());
        for link in links {
            if self.queued.insert(link.as_str().to_owned()) {
                self.frontier.push_back(HtmlCrawlRequest {
                    url: link,
                    depth: request.depth.saturating_add(1),
                });
            }
        }
        self.pages.push(request);
        Ok(())
    }

    #[must_use]
    pub fn has_pending_pages(&self) -> bool {
        !self.frontier.is_empty() || !self.in_flight.is_empty()
    }

    /// Finishes the crawl after every issued and discovered page has been submitted.
    ///
    /// # Errors
    /// Returns [`HtmlCrawlError::PendingPages`] while work remains.
    pub fn finish(self) -> Result<HtmlCrawlReport, HtmlCrawlError> {
        if self.has_pending_pages() {
            Err(HtmlCrawlError::PendingPages)
        } else {
            Ok(HtmlCrawlReport { pages: self.pages })
        }
    }
}

/// Resolves and canonicalizes an HTTP(S) reference for crawler discovery.
///
/// Fragments are removed and URL-library normalization covers dot segments, host casing, empty
/// paths, percent escapes, and default ports. Credential-bearing and non-HTTP(S) references are
/// rejected.
#[must_use]
pub fn canonicalize_http_reference(base_url: &HttpUrl, reference: &str) -> Option<HttpUrl> {
    let base = Url::parse(base_url.as_str()).ok()?;
    let mut resolved = base.join(reference).ok()?;
    if !is_http_url(&resolved) || !resolved.username().is_empty() || resolved.password().is_some() {
        return None;
    }
    resolved.set_fragment(None);
    HttpUrl::parse(resolved.as_str()).ok()
}

/// Crawls static HTML pages reachable from `start_url` on the exact same host.
///
/// The caller owns transport and supplies each page body through `fetch_html`. Subdomains and
/// non-HTTP(S) links are ignored. Parser limits are defense-in-depth ceilings; configurable caps
/// belong to the separate crawl-cap policy.
///
/// # Errors
/// Returns a typed failure when fetching fails or a parser safety ceiling is reached.
pub fn crawl_same_host_html<F, E>(
    start_url: &HttpUrl,
    mut fetch_html: F,
) -> Result<HtmlCrawlReport, HtmlCrawlError>
where
    F: FnMut(&HttpUrl) -> Result<String, E>,
    E: Display,
{
    let mut crawler = SameHostHtmlCrawler::new(start_url)?;
    while let Some(request) = crawler.next_request()? {
        let html = fetch_html(&request.url).map_err(|error| HtmlCrawlError::Fetch {
            url: request.url.clone(),
            message: error.to_string(),
        })?;
        crawler.submit_html(request, &html)?;
    }
    crawler.finish()
}

fn discover_html_links(
    page_url: &HttpUrl,
    html: &str,
    crawl_host: &str,
) -> Result<Vec<HttpUrl>, HtmlCrawlError> {
    let document = Html::parse_document(html);
    let base_selector = Selector::parse("base[href]").expect("static selector is valid");
    let link_selector = Selector::parse("a[href]").expect("static selector is valid");
    let base_url = document
        .select(&base_selector)
        .next()
        .and_then(|element| element.value().attr("href"))
        .and_then(|href| canonicalize_http_reference(page_url, href))
        .unwrap_or_else(|| page_url.clone());

    let mut links = Vec::new();
    for element in document.select(&link_selector) {
        let Some(href) = element.value().attr("href") else {
            continue;
        };
        let Some(link) = canonicalize_http_reference(&base_url, href) else {
            continue;
        };
        let Ok(parsed) = Url::parse(link.as_str()) else {
            continue;
        };
        if parsed.host_str() != Some(crawl_host) {
            continue;
        }
        links.push(link);
        if links.len() > MAX_LINKS_PER_PAGE {
            return Err(HtmlCrawlError::TooManyLinks {
                url: page_url.clone(),
            });
        }
    }
    Ok(links)
}

fn is_http_url(url: &Url) -> bool {
    matches!(url.scheme(), "http" | "https") && url.host().is_some()
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::{HtmlCrawlError, canonicalize_http_reference, crawl_same_host_html};
    use crate::domain::HttpUrl;

    fn url(value: &str) -> HttpUrl {
        HttpUrl::parse(value).expect("fixture URL should be valid")
    }

    #[test]
    fn cyclic_relative_and_base_urls_are_canonicalized_and_visited_once() {
        let pages = HashMap::from([
            (
                "https://example.test/docs/index.html",
                r#"
                    <html><head><base href="/mirror/"></head><body>
                      <a href="chapter.html#one">chapter</a>
                      <a href="./chapter.html#two">duplicate fragment</a>
                      <a href="../docs/index.html#again">cycle</a>
                      <a href="https://outside.test/page.html">outside</a>
                      <a href="mailto:owner@example.test">mail</a>
                    </body></html>
                "#,
            ),
            (
                "https://example.test/mirror/chapter.html",
                r#"
                    <a href="../final.html">final</a>
                    <a href="../docs/index.html">back</a>
                "#,
            ),
            (
                "https://example.test/final.html",
                r#"<a href="/mirror/chapter.html">cycle again</a>"#,
            ),
        ]);

        let report = crawl_same_host_html(
            &url("https://example.test/docs/index.html#start"),
            |requested| {
                pages
                    .get(requested.as_str())
                    .map(ToString::to_string)
                    .ok_or_else(|| format!("unexpected URL: {requested}"))
            },
        )
        .expect("fixture crawl should finish");

        let visited: Vec<_> = report
            .pages
            .iter()
            .map(|page| (page.url.as_str(), page.depth))
            .collect();
        assert_eq!(
            visited,
            vec![
                ("https://example.test/docs/index.html", 0),
                ("https://example.test/mirror/chapter.html", 1),
                ("https://example.test/final.html", 2),
            ]
        );
    }

    #[test]
    fn exact_host_boundary_rejects_subdomains_credentials_and_other_schemes() {
        let start = url("https://example.test/");
        let report = crawl_same_host_html(&start, |requested| {
            assert_eq!(requested, &start);
            Ok::<_, String>(
                r#"
                    <a href="https://cdn.example.test/a">subdomain</a>
                    <a href="https://user:secret@example.test/private">credentials</a>
                    <a href="javascript:alert(1)">script</a>
                "#
                .to_owned(),
            )
        })
        .expect("excluded URLs must not be fetched");

        assert_eq!(report.pages.len(), 1);
        assert_eq!(report.pages[0].url, start);
    }

    #[test]
    fn canonical_helper_normalizes_fragments_paths_hosts_and_default_ports() {
        let normalized = canonicalize_http_reference(
            &url("https://EXAMPLE.test:443/a/b/index.html"),
            "../asset/./page.html#section",
        )
        .expect("reference should be crawlable");

        assert_eq!(
            normalized.as_str(),
            "https://example.test/a/asset/page.html"
        );
    }

    #[test]
    fn fetch_failure_names_the_canonical_page() {
        let error = crawl_same_host_html(&url("https://example.test/#fragment"), |_| {
            Err::<String, _>("offline")
        })
        .expect_err("fetch failure should stop the crawl");

        assert_eq!(
            error,
            HtmlCrawlError::Fetch {
                url: url("https://example.test/"),
                message: "offline".to_owned(),
            }
        );
    }
}
