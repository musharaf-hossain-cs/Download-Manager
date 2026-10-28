//! Bounded CSS asset discovery for the site grabber.

use std::collections::HashSet;

use thiserror::Error;
use url::Url;

const MAX_STYLESHEET_BYTES: usize = 1024 * 1024;
const MAX_DISCOVERED_URLS: usize = 4096;

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum CssDiscoveryError {
    #[error("stylesheet URL must use HTTP or HTTPS and contain a host")]
    InvalidStylesheetUrl,
    #[error("stylesheet exceeds the 1 MiB discovery limit")]
    StylesheetTooLarge,
    #[error("stylesheet contains more than 4096 unique same-host URLs")]
    TooManyUrls,
}

/// Discovers canonical, unique, same-host `url()` and `@import` references from CSS.
///
/// Relative references are resolved against `stylesheet_url`. Fragments are removed before
/// deduplication, and non-HTTP(S), cross-host, empty, and malformed references are ignored.
/// Comments and ordinary quoted strings are skipped so text that merely resembles CSS syntax is
/// not treated as a resource.
///
/// # Errors
/// Returns an error for an invalid stylesheet URL, an oversized stylesheet, or excessive unique
/// references.
pub fn discover_css_assets(
    stylesheet_url: &Url,
    stylesheet: &str,
) -> Result<Vec<Url>, CssDiscoveryError> {
    validate_stylesheet_url(stylesheet_url)?;
    if stylesheet.len() > MAX_STYLESHEET_BYTES {
        return Err(CssDiscoveryError::StylesheetTooLarge);
    }

    let bytes = stylesheet.as_bytes();
    let mut index = 0;
    let mut seen = HashSet::new();
    let mut discovered = Vec::new();
    while index < bytes.len() {
        if starts_with(bytes, index, b"/*") {
            index = skip_comment(bytes, index + 2);
            continue;
        }
        if matches!(bytes[index], b'\'' | b'"') {
            index = skip_quoted(bytes, index).1;
            continue;
        }
        if starts_ascii_case_insensitive(bytes, index, b"@import")
            && token_ends_at(bytes, index + b"@import".len())
        {
            index += b"@import".len();
            index = skip_css_space_and_comments(bytes, index);
            if index < bytes.len() && matches!(bytes[index], b'\'' | b'"') {
                let (reference, next) = read_quoted(bytes, index);
                push_reference(stylesheet_url, reference, &mut seen, &mut discovered)?;
                index = next;
                continue;
            }
        }
        if starts_ascii_case_insensitive(bytes, index, b"url") && token_starts_at(bytes, index) {
            let after_name = skip_ascii_whitespace(bytes, index + b"url".len());
            if bytes.get(after_name) == Some(&b'(') {
                let (reference, next) = read_url_function(bytes, after_name + 1);
                push_reference(stylesheet_url, reference, &mut seen, &mut discovered)?;
                index = next;
                continue;
            }
        }
        index += 1;
    }
    Ok(discovered)
}

fn validate_stylesheet_url(url: &Url) -> Result<(), CssDiscoveryError> {
    if matches!(url.scheme(), "http" | "https") && url.host_str().is_some() {
        Ok(())
    } else {
        Err(CssDiscoveryError::InvalidStylesheetUrl)
    }
}

fn push_reference(
    base: &Url,
    reference: Option<&str>,
    seen: &mut HashSet<String>,
    discovered: &mut Vec<Url>,
) -> Result<(), CssDiscoveryError> {
    let Some(reference) = reference.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(());
    };
    let Ok(mut resolved) = base.join(reference) else {
        return Ok(());
    };
    if !matches!(resolved.scheme(), "http" | "https") || resolved.host_str() != base.host_str() {
        return Ok(());
    }
    resolved.set_fragment(None);
    if seen.insert(resolved.as_str().to_owned()) {
        if discovered.len() == MAX_DISCOVERED_URLS {
            return Err(CssDiscoveryError::TooManyUrls);
        }
        discovered.push(resolved);
    }
    Ok(())
}

fn read_url_function(bytes: &[u8], mut index: usize) -> (Option<&str>, usize) {
    index = skip_ascii_whitespace(bytes, index);
    if index >= bytes.len() {
        return (None, index);
    }
    if matches!(bytes[index], b'\'' | b'"') {
        let (value, next) = read_quoted(bytes, index);
        let next = skip_ascii_whitespace(bytes, next);
        return (value, consume_closing_parenthesis(bytes, next));
    }

    let start = index;
    while index < bytes.len() && bytes[index] != b')' {
        if bytes[index] == b'\\' && index + 1 < bytes.len() {
            index += 2;
        } else {
            index += 1;
        }
    }
    let value = std::str::from_utf8(&bytes[start..index]).ok();
    (value, consume_closing_parenthesis(bytes, index))
}

fn read_quoted(bytes: &[u8], start: usize) -> (Option<&str>, usize) {
    let quote = bytes[start];
    let mut index = start + 1;
    while index < bytes.len() {
        match bytes[index] {
            b'\\' if index + 1 < bytes.len() => index += 2,
            value if value == quote => {
                return (
                    std::str::from_utf8(&bytes[start + 1..index]).ok(),
                    index + 1,
                );
            }
            _ => index += 1,
        }
    }
    (None, bytes.len())
}

fn skip_quoted(bytes: &[u8], start: usize) -> (Option<&str>, usize) {
    read_quoted(bytes, start)
}

fn skip_comment(bytes: &[u8], mut index: usize) -> usize {
    while index + 1 < bytes.len() {
        if starts_with(bytes, index, b"*/") {
            return index + 2;
        }
        index += 1;
    }
    bytes.len()
}

fn skip_css_space_and_comments(bytes: &[u8], mut index: usize) -> usize {
    loop {
        index = skip_ascii_whitespace(bytes, index);
        if starts_with(bytes, index, b"/*") {
            index = skip_comment(bytes, index + 2);
        } else {
            return index;
        }
    }
}

fn skip_ascii_whitespace(bytes: &[u8], mut index: usize) -> usize {
    while bytes.get(index).is_some_and(u8::is_ascii_whitespace) {
        index += 1;
    }
    index
}

fn consume_closing_parenthesis(bytes: &[u8], index: usize) -> usize {
    if bytes.get(index) == Some(&b')') {
        index + 1
    } else {
        index
    }
}

fn starts_with(bytes: &[u8], index: usize, needle: &[u8]) -> bool {
    bytes.get(index..index.saturating_add(needle.len())) == Some(needle)
}

fn starts_ascii_case_insensitive(bytes: &[u8], index: usize, needle: &[u8]) -> bool {
    bytes
        .get(index..index.saturating_add(needle.len()))
        .is_some_and(|value| value.eq_ignore_ascii_case(needle))
}

fn token_starts_at(bytes: &[u8], index: usize) -> bool {
    index == 0 || !is_css_identifier_byte(bytes[index - 1])
}

fn token_ends_at(bytes: &[u8], index: usize) -> bool {
    bytes
        .get(index)
        .is_none_or(|value| !is_css_identifier_byte(*value))
}

const fn is_css_identifier_byte(value: u8) -> bool {
    value.is_ascii_alphanumeric() || matches!(value, b'-' | b'_')
}

#[cfg(test)]
mod tests {
    use super::{CssDiscoveryError, discover_css_assets};
    use url::Url;

    #[test]
    fn imports_urls_and_canonical_duplicates_are_discovered_once() {
        let base = Url::parse("https://example.test/css/site/main.css").expect("base URL");
        let css = r#"
            @import "../theme.css#top" screen;
            @IMPORT url('../theme.css');
            .hero { background: URL(../../img/hero.webp?size=2#first); }
            .same { src: url("/img/../img/hero.webp?size=2#second"); }
            .font { src: url('/fonts/site.woff2') format('woff2'); }
        "#;

        let found = discover_css_assets(&base, css).expect("discover assets");
        let values: Vec<_> = found.iter().map(Url::as_str).collect();
        assert_eq!(
            values,
            [
                "https://example.test/css/theme.css",
                "https://example.test/img/hero.webp?size=2",
                "https://example.test/fonts/site.woff2",
            ]
        );
    }

    #[test]
    fn comments_strings_cross_host_and_non_http_references_are_ignored() {
        let base = Url::parse("https://example.test/site.css").expect("base URL");
        let css = r#"
            /* url('/comment.png') */
            .label::after { content: "url('/text.png')"; }
            .external { background: url(https://cdn.example.test/a.png); }
            .inline { background: url(data:image/png;base64,AAAA); }
            .empty { background: url( ); }
            .safe { background: url('/safe.png'); }
        "#;

        assert_eq!(
            discover_css_assets(&base, css).expect("discover assets"),
            [Url::parse("https://example.test/safe.png").expect("safe URL")]
        );
    }

    #[test]
    fn malformed_and_oversized_input_is_bounded() {
        let base = Url::parse("https://example.test/site.css").expect("base URL");
        assert!(discover_css_assets(&base, ".a { background: url('unterminated }").is_ok());
        assert_eq!(
            discover_css_assets(&Url::parse("file:///tmp/site.css").expect("file URL"), ""),
            Err(CssDiscoveryError::InvalidStylesheetUrl)
        );
        assert_eq!(
            discover_css_assets(&base, &"x".repeat(1024 * 1024 + 1)),
            Err(CssDiscoveryError::StylesheetTooLarge)
        );
    }
}
