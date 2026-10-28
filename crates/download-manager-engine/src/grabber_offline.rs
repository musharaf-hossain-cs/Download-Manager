//! Safe local paths and offline reference rewriting for static grabber projects.

use std::path::{Component, Path, PathBuf};

use percent_encoding::percent_decode_str;
use scraper::{Html, Node, Selector};
use sha2::{Digest, Sha256};
use thiserror::Error;
use url::Url;

use crate::{domain::HttpUrl, filename::sanitize_windows_filename};

const MAX_DOCUMENT_BYTES: usize = 8 * 1024 * 1024;
const LINK_ATTRIBUTES: [&str; 4] = ["href", "src", "poster", "data"];
const HEX: &[u8; 16] = b"0123456789abcdef";

/// A URL mapped to a path that is lexically contained by the selected project directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OfflinePath {
    pub relative: PathBuf,
    pub absolute: PathBuf,
}

/// Deterministic same-host URL-to-file mapping for one offline project.
#[derive(Debug, Clone)]
pub struct OfflineMapper {
    destination: PathBuf,
    host: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum OfflineRewriteError {
    #[error("offline destination must be an absolute path without parent traversal")]
    UnsafeDestination,
    #[error("URL is outside the offline project's exact host")]
    OutsideHost,
    #[error("URL could not be mapped beneath the offline destination")]
    UnsafeMapping,
    #[error("document exceeds the 8 MiB offline rewrite limit")]
    DocumentTooLarge,
}

impl OfflineMapper {
    /// Creates a mapper rooted at an absolute, traversal-free destination.
    ///
    /// # Errors
    /// Rejects relative destinations and destinations containing `..` components.
    pub fn new(
        destination: impl Into<PathBuf>,
        start_url: &HttpUrl,
    ) -> Result<Self, OfflineRewriteError> {
        let destination = destination.into();
        if !destination.is_absolute()
            || destination
                .components()
                .any(|component| matches!(component, Component::ParentDir))
        {
            return Err(OfflineRewriteError::UnsafeDestination);
        }
        let parsed =
            Url::parse(start_url.as_str()).map_err(|_| OfflineRewriteError::UnsafeMapping)?;
        let host = parsed
            .host_str()
            .ok_or(OfflineRewriteError::UnsafeMapping)?
            .to_owned();
        Ok(Self { destination, host })
    }

    /// Maps one exact-host HTTP(S) URL to a contained local path.
    ///
    /// Query strings receive a stable suffix so distinct remote objects cannot overwrite each
    /// other. Fragments never affect the saved object path.
    ///
    /// # Errors
    /// Rejects URLs outside the project's host or any mapping that is not beneath the destination.
    pub fn map_url(&self, url: &HttpUrl) -> Result<OfflinePath, OfflineRewriteError> {
        let parsed = Url::parse(url.as_str()).map_err(|_| OfflineRewriteError::UnsafeMapping)?;
        if parsed.host_str() != Some(self.host.as_str()) {
            return Err(OfflineRewriteError::OutsideHost);
        }

        let authority = format!(
            "{}_{}_{}",
            parsed.scheme(),
            self.host,
            parsed.port_or_known_default().unwrap_or_default()
        );
        let mut components = vec![sanitize_windows_filename(&authority)];
        if let Some(segments) = parsed.path_segments() {
            components.extend(
                segments
                    .filter(|segment| !segment.is_empty())
                    .map(|segment| {
                        let decoded = percent_decode_str(segment).decode_utf8_lossy();
                        sanitize_windows_filename(&decoded)
                    }),
            );
        }
        if parsed.path().ends_with('/') || components.len() == 1 {
            components.push("index.html".to_owned());
        }
        if let Some(query) = parsed.query() {
            let last = components
                .last_mut()
                .ok_or(OfflineRewriteError::UnsafeMapping)?;
            *last = add_identity_suffix(last, query);
        }

        let relative = components
            .iter()
            .fold(PathBuf::new(), |path, part| path.join(part));
        if relative
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
        {
            return Err(OfflineRewriteError::UnsafeMapping);
        }
        let absolute = self.destination.join(&relative);
        if !absolute.starts_with(&self.destination) {
            return Err(OfflineRewriteError::UnsafeMapping);
        }
        Ok(OfflinePath { relative, absolute })
    }

    fn local_reference(
        &self,
        from_url: &HttpUrl,
        target: &Url,
    ) -> Result<String, OfflineRewriteError> {
        let target_url =
            HttpUrl::parse(target.as_str()).map_err(|_| OfflineRewriteError::UnsafeMapping)?;
        let from = self.map_url(from_url)?;
        let to = self.map_url(&target_url)?;
        let from_parts = normal_parts(from.relative.parent().unwrap_or(Path::new("")))?;
        let to_parts = normal_parts(&to.relative)?;
        let shared = from_parts
            .iter()
            .zip(&to_parts)
            .take_while(|(left, right)| left == right)
            .count();
        let mut parts = vec!["..".to_owned(); from_parts.len() - shared];
        parts.extend(to_parts[shared..].iter().cloned());
        let mut reference = if parts.is_empty() {
            "./".to_owned()
        } else {
            parts.join("/")
        };
        if let Some(fragment) = target.fragment() {
            reference.push('#');
            reference.push_str(fragment);
        }
        Ok(reference)
    }
}

/// Rewrites same-host HTML link attributes to contained local references.
///
/// A remote `<base>` is used for resolution and then neutralized, preventing an offline browser
/// from sending rewritten relative references back to the network.
///
/// # Errors
/// Returns a mapping error or rejects documents over the parser ceiling.
pub fn rewrite_html_offline(
    mapper: &OfflineMapper,
    page_url: &HttpUrl,
    html: &str,
) -> Result<String, OfflineRewriteError> {
    ensure_document_size(html)?;
    mapper.map_url(page_url)?;
    let mut document = Html::parse_document(html);
    let selector = Selector::parse("base[href]").map_err(|_| OfflineRewriteError::UnsafeMapping)?;
    let default_base =
        Url::parse(page_url.as_str()).map_err(|_| OfflineRewriteError::UnsafeMapping)?;
    let effective_base = document
        .select(&selector)
        .next()
        .and_then(|element| element.value().attr("href"))
        .and_then(|reference| Url::parse(page_url.as_str()).ok()?.join(reference).ok())
        .unwrap_or(default_base);

    for node in document.tree.values_mut() {
        let Node::Element(element) = node else {
            continue;
        };
        let is_base = element.name() == "base";
        for (name, value) in &mut element.attrs {
            let attribute: &str = &name.local;
            if attribute == "href" && is_base {
                *value = "./".into();
                continue;
            }
            if !LINK_ATTRIBUTES.contains(&attribute) {
                continue;
            }
            if let Some(rewritten) = rewrite_reference(mapper, page_url, &effective_base, value) {
                *value = rewritten.into();
            }
        }
    }
    Ok(document.html())
}

/// Rewrites same-host CSS `url()` and `@import` references to contained local references.
///
/// # Errors
/// Returns a mapping error or rejects stylesheets over the parser ceiling.
pub fn rewrite_css_offline(
    mapper: &OfflineMapper,
    stylesheet_url: &HttpUrl,
    stylesheet: &str,
) -> Result<String, OfflineRewriteError> {
    ensure_document_size(stylesheet)?;
    mapper.map_url(stylesheet_url)?;
    let base =
        Url::parse(stylesheet_url.as_str()).map_err(|_| OfflineRewriteError::UnsafeMapping)?;
    let bytes = stylesheet.as_bytes();
    let mut replacements = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        if bytes.get(index..index + 2) == Some(b"/*") {
            index = find_after(bytes, index + 2, b"*/");
            continue;
        }
        if starts_keyword(bytes, index, b"url") {
            let open = skip_space(bytes, index + 3);
            if bytes.get(open) == Some(&b'(') {
                let (start, end, next) = css_value(bytes, open + 1, b')');
                push_css_replacement(
                    mapper,
                    stylesheet_url,
                    &base,
                    stylesheet,
                    start,
                    end,
                    &mut replacements,
                );
                index = next;
                continue;
            }
        }
        if starts_keyword(bytes, index, b"@import") {
            let start = skip_space(bytes, index + 7);
            if bytes
                .get(start)
                .is_some_and(|byte| matches!(byte, b'\'' | b'"'))
            {
                let quote = bytes[start];
                let (value_start, value_end, next) = css_value(bytes, start + 1, quote);
                push_css_replacement(
                    mapper,
                    stylesheet_url,
                    &base,
                    stylesheet,
                    value_start,
                    value_end,
                    &mut replacements,
                );
                index = next;
                continue;
            }
        }
        if matches!(bytes[index], b'\'' | b'"') {
            let (_, _, next) = css_value(bytes, index + 1, bytes[index]);
            index = next;
        } else {
            index += 1;
        }
    }
    Ok(apply_replacements(stylesheet, replacements))
}

fn rewrite_reference(
    mapper: &OfflineMapper,
    source: &HttpUrl,
    base: &Url,
    reference: &str,
) -> Option<String> {
    let target = base.join(reference).ok()?;
    if !matches!(target.scheme(), "http" | "https") {
        return None;
    }
    if target.host_str() == Some(mapper.host.as_str())
        && target.username().is_empty()
        && target.password().is_none()
    {
        mapper.local_reference(source, &target).ok()
    } else {
        Some(target.into())
    }
}

fn push_css_replacement(
    mapper: &OfflineMapper,
    source: &HttpUrl,
    base: &Url,
    input: &str,
    start: usize,
    end: usize,
    replacements: &mut Vec<(usize, usize, String)>,
) {
    if start < end
        && let Some(rewritten) = rewrite_reference(mapper, source, base, input[start..end].trim())
    {
        let leading = input[start..end].len() - input[start..end].trim_start().len();
        let trimmed_end = end - (input[start..end].len() - input[start..end].trim_end().len());
        replacements.push((start + leading, trimmed_end, rewritten));
    }
}

fn css_value(bytes: &[u8], start: usize, terminator: u8) -> (usize, usize, usize) {
    let mut value_start = skip_space(bytes, start);
    let quote = bytes
        .get(value_start)
        .copied()
        .filter(|byte| matches!(byte, b'\'' | b'"'));
    if quote.is_some() {
        value_start += 1;
    }
    let expected = quote.unwrap_or(terminator);
    let mut index = value_start;
    while index < bytes.len() && bytes[index] != expected {
        index += if bytes[index] == b'\\' && index + 1 < bytes.len() {
            2
        } else {
            1
        };
    }
    let value_end = index;
    if quote.is_some() && index < bytes.len() {
        index += 1;
    }
    if terminator == b')' {
        index = skip_space(bytes, index);
        if bytes.get(index) == Some(&b')') {
            index += 1;
        }
    } else if index < bytes.len() {
        index += 1;
    }
    (value_start, value_end, index)
}

fn starts_keyword(bytes: &[u8], index: usize, keyword: &[u8]) -> bool {
    bytes
        .get(index..index + keyword.len())
        .is_some_and(|value| value.eq_ignore_ascii_case(keyword))
        && (index == 0 || !bytes[index - 1].is_ascii_alphanumeric())
}

fn skip_space(bytes: &[u8], mut index: usize) -> usize {
    while bytes.get(index).is_some_and(u8::is_ascii_whitespace) {
        index += 1;
    }
    index
}

fn find_after(bytes: &[u8], mut index: usize, needle: &[u8]) -> usize {
    while index + needle.len() <= bytes.len() {
        if &bytes[index..index + needle.len()] == needle {
            return index + needle.len();
        }
        index += 1;
    }
    bytes.len()
}

fn apply_replacements(input: &str, replacements: Vec<(usize, usize, String)>) -> String {
    let mut output = String::with_capacity(input.len());
    let mut copied = 0;
    for (start, end, replacement) in replacements {
        output.push_str(&input[copied..start]);
        output.push_str(&replacement);
        copied = end;
    }
    output.push_str(&input[copied..]);
    output
}

fn ensure_document_size(document: &str) -> Result<(), OfflineRewriteError> {
    if document.len() > MAX_DOCUMENT_BYTES {
        Err(OfflineRewriteError::DocumentTooLarge)
    } else {
        Ok(())
    }
}

fn normal_parts(path: &Path) -> Result<Vec<String>, OfflineRewriteError> {
    path.components()
        .map(|component| match component {
            Component::Normal(value) => value
                .to_str()
                .map(str::to_owned)
                .ok_or(OfflineRewriteError::UnsafeMapping),
            _ => Err(OfflineRewriteError::UnsafeMapping),
        })
        .collect()
}

fn add_identity_suffix(filename: &str, query: &str) -> String {
    let digest = Sha256::digest(query.as_bytes());
    let mut suffix = String::with_capacity(12);
    for byte in &digest[..6] {
        suffix.push(char::from(HEX[usize::from(byte >> 4)]));
        suffix.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    match filename.rfind('.').filter(|index| *index > 0) {
        Some(index) => format!("{}__q-{suffix}{}", &filename[..index], &filename[index..]),
        None => format!("{filename}__q-{suffix}"),
    }
}

#[cfg(test)]
mod tests {
    use std::path::{Component, Path, PathBuf};

    use scraper::{Html, Selector};
    use tempfile::tempdir;

    use super::{OfflineMapper, OfflineRewriteError, rewrite_css_offline, rewrite_html_offline};
    use crate::domain::HttpUrl;

    fn url(value: &str) -> HttpUrl {
        HttpUrl::parse(value).expect("fixture URL")
    }

    #[test]
    fn mapping_sanitizes_traversal_devices_and_query_collisions_beneath_destination() {
        let directory = tempdir().expect("temporary destination");
        let mapper =
            OfflineMapper::new(directory.path(), &url("https://example.test/")).expect("mapper");
        let first = mapper
            .map_url(&url(
                "https://example.test/%2e%2e/CON/report.html?version=1",
            ))
            .expect("first mapping");
        let second = mapper
            .map_url(&url(
                "https://example.test/%2e%2e/CON/report.html?version=2",
            ))
            .expect("second mapping");

        assert!(first.absolute.starts_with(directory.path()));
        assert!(
            first
                .relative
                .components()
                .all(|part| matches!(part, Component::Normal(_)))
        );
        assert_ne!(first.relative, second.relative);
        assert!(!first.relative.to_string_lossy().contains(".."));
        assert!(first.relative.to_string_lossy().contains("_CON"));
        assert_eq!(
            mapper.map_url(&url("https://outside.test/file")),
            Err(OfflineRewriteError::OutsideHost)
        );
    }

    #[test]
    fn html_and_css_references_resolve_inside_destination() {
        let directory = tempdir().expect("temporary destination");
        let page = url("https://example.test/docs/index.html");
        let mapper = OfflineMapper::new(directory.path(), &page).expect("mapper");
        let html = rewrite_html_offline(
            &mapper,
            &page,
            r#"<base href="/assets/"><a href="../guide/page.html#intro">Guide</a>
                <img src="img/logo.png?size=2"><script src="https://outside.test/app.js"></script>"#,
        )
        .expect("HTML rewrite");
        let document = Html::parse_document(&html);
        let selector = Selector::parse("[href], [src]").expect("selector");
        let page_path = mapper.map_url(&page).expect("page mapping").absolute;
        for element in document.select(&selector) {
            for attribute in ["href", "src"] {
                let Some(reference) = element.value().attr(attribute) else {
                    continue;
                };
                if reference.starts_with("https://outside.test") {
                    continue;
                }
                assert_reference_is_contained(directory.path(), &page_path, reference);
            }
        }
        assert!(html.contains("https://outside.test/app.js"));
        assert!(!html.contains("href=\"/assets/\""));

        let external_base = rewrite_html_offline(
            &mapper,
            &page,
            r#"<base href="https://outside.test/root/"><img src="relative.png">"#,
        )
        .expect("external base rewrite");
        assert!(external_base.contains("src=\"https://outside.test/root/relative.png\""));
        assert!(external_base.contains("base href=\"./\""));

        let stylesheet = url("https://example.test/css/site/main.css");
        let css = rewrite_css_offline(
            &mapper,
            &stylesheet,
            r#"@import "../theme.css"; .hero { background: url('../../img/hero.png#wide') }
               .external { background: url(https://cdn.test/x.png) } /* url('../ignored.png') */"#,
        )
        .expect("CSS rewrite");
        let stylesheet_path = mapper.map_url(&stylesheet).expect("CSS mapping").absolute;
        for reference in extract_css_references(&css) {
            if reference.starts_with("https://cdn.test") || reference.contains("ignored.png") {
                continue;
            }
            assert_reference_is_contained(directory.path(), &stylesheet_path, reference);
        }
        assert!(css.contains("https://cdn.test/x.png"));
        assert!(css.contains("ignored.png"));
    }

    fn assert_reference_is_contained(root: &Path, source: &Path, reference: &str) {
        let path = reference.split('#').next().expect("path part");
        let joined = source
            .parent()
            .expect("source parent")
            .join(path.replace('/', "\\"));
        let normalized = lexical_normalize(&joined);
        assert!(normalized.starts_with(root), "{reference} escaped {root:?}");
    }

    fn lexical_normalize(path: &Path) -> PathBuf {
        let mut normalized = PathBuf::new();
        for component in path.components() {
            if component == Component::ParentDir {
                normalized.pop();
            } else {
                normalized.push(component);
            }
        }
        normalized
    }

    fn extract_css_references(css: &str) -> Vec<&str> {
        css.split(['\'', '"', '(', ')'])
            .map(str::trim)
            .filter(|value| value.contains(".css") || value.contains(".png"))
            .collect()
    }
}
