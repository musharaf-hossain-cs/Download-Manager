//! RFC-aware filename selection and Windows-safe destination construction.

use std::path::{Path, PathBuf};

use percent_encoding::percent_decode_str;

use crate::domain::HttpUrl;

const DEFAULT_FILENAME: &str = "download.bin";
const MAX_FILENAME_CHARS: usize = 240;

/// Selects a filename using RFC 5987 `filename*`, legacy `filename`, URL basename, then MIME.
#[must_use]
pub fn select_filename(
    content_disposition: Option<&str>,
    final_url: &HttpUrl,
    mime_type: Option<&str>,
) -> String {
    content_disposition
        .and_then(disposition_filename)
        .or_else(|| url_basename(final_url))
        .or_else(|| mime_fallback(mime_type).map(str::to_owned))
        .unwrap_or_else(|| DEFAULT_FILENAME.to_owned())
}

/// Converts an untrusted suggested filename to one safe Windows path component.
#[must_use]
pub fn sanitize_windows_filename(input: &str) -> String {
    let mut sanitized = input
        .chars()
        .map(|character| {
            if character.is_control()
                || matches!(
                    character,
                    '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*'
                )
            {
                '_'
            } else {
                character
            }
        })
        .collect::<String>();
    while sanitized.contains("..") {
        sanitized = sanitized.replace("..", "__");
    }
    sanitized = sanitized.trim().trim_end_matches(['.', ' ']).to_owned();
    if sanitized.is_empty() {
        sanitized.push_str(DEFAULT_FILENAME);
    }
    if is_windows_device_name(&sanitized) {
        sanitized.insert(0, '_');
    }
    truncate_chars(&mut sanitized, MAX_FILENAME_CHARS);
    sanitized
}

/// Joins a sanitized single component beneath the user-selected destination directory.
#[must_use]
pub fn safe_destination(directory: &Path, untrusted_filename: &str) -> PathBuf {
    directory.join(sanitize_windows_filename(untrusted_filename))
}

fn disposition_filename(value: &str) -> Option<String> {
    let parameters = split_parameters(value);
    parameters
        .iter()
        .find_map(|parameter| parse_parameter(parameter, "filename*").and_then(decode_extended))
        .or_else(|| {
            parameters
                .iter()
                .find_map(|parameter| parse_parameter(parameter, "filename").and_then(unquote))
        })
        .filter(|filename| !filename.trim().is_empty())
}

fn split_parameters(value: &str) -> Vec<String> {
    let mut parameters = Vec::new();
    let mut current = String::new();
    let mut quoted = false;
    let mut escaped = false;
    for character in value.chars() {
        if escaped {
            current.push(character);
            escaped = false;
        } else if character == '\\' && quoted {
            current.push(character);
            escaped = true;
        } else if character == '"' {
            quoted = !quoted;
            current.push(character);
        } else if character == ';' && !quoted {
            parameters.push(current.trim().to_owned());
            current.clear();
        } else {
            current.push(character);
        }
    }
    parameters.push(current.trim().to_owned());
    parameters
}

fn parse_parameter<'a>(parameter: &'a str, expected: &str) -> Option<&'a str> {
    let (name, value) = parameter.split_once('=')?;
    name.trim()
        .eq_ignore_ascii_case(expected)
        .then_some(value.trim())
}

fn decode_extended(value: &str) -> Option<String> {
    let value = unquote(value)?;
    let (charset, remainder) = value.split_once('\'')?;
    let (_, encoded) = remainder.split_once('\'')?;
    let bytes = percent_decode_str(encoded).collect::<Vec<_>>();
    if charset.eq_ignore_ascii_case("utf-8") {
        String::from_utf8(bytes).ok()
    } else if charset.eq_ignore_ascii_case("iso-8859-1") {
        Some(bytes.into_iter().map(char::from).collect())
    } else {
        None
    }
}

fn unquote(value: &str) -> Option<String> {
    let value = value.trim();
    if value.starts_with('"') {
        let inner = value.strip_prefix('"')?.strip_suffix('"')?;
        let mut output = String::new();
        let mut escaped = false;
        for character in inner.chars() {
            if escaped {
                output.push(character);
                escaped = false;
            } else if character == '\\' {
                escaped = true;
            } else {
                output.push(character);
            }
        }
        if escaped {
            output.push('\\');
        }
        Some(output)
    } else {
        Some(value.to_owned())
    }
}

fn url_basename(url: &HttpUrl) -> Option<String> {
    let parsed = url::Url::parse(url.as_str()).ok()?;
    let segment = parsed
        .path_segments()?
        .next_back()
        .filter(|segment| !segment.is_empty())?;
    percent_decode_str(segment)
        .decode_utf8()
        .ok()
        .map(Into::into)
}

fn mime_fallback(mime_type: Option<&str>) -> Option<&'static str> {
    match mime_type {
        Some("text/html") => Some("download.html"),
        Some("text/plain") => Some("download.txt"),
        Some("application/json") => Some("download.json"),
        Some("application/pdf") => Some("download.pdf"),
        Some("image/jpeg") => Some("download.jpg"),
        Some("image/png") => Some("download.png"),
        Some("video/mp4") => Some("download.mp4"),
        Some("audio/mpeg") => Some("download.mp3"),
        Some("application/zip") => Some("download.zip"),
        Some(_) => Some(DEFAULT_FILENAME),
        None => None,
    }
}

fn is_windows_device_name(filename: &str) -> bool {
    let stem = filename.split('.').next().unwrap_or(filename);
    let upper = stem.trim_end_matches(['.', ' ']).to_ascii_uppercase();
    matches!(upper.as_str(), "CON" | "PRN" | "AUX" | "NUL" | "CLOCK$")
        || numbered_device(&upper, "COM")
        || numbered_device(&upper, "LPT")
}

fn numbered_device(value: &str, prefix: &str) -> bool {
    value
        .strip_prefix(prefix)
        .is_some_and(|suffix| matches!(suffix, "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9"))
}

fn truncate_chars(value: &mut String, maximum: usize) {
    if let Some((index, _)) = value.char_indices().nth(maximum) {
        value.truncate(index);
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{safe_destination, sanitize_windows_filename, select_filename};
    use crate::domain::HttpUrl;

    fn url(value: &str) -> HttpUrl {
        HttpUrl::parse(value).expect("URL")
    }

    #[test]
    fn filename_selection_prefers_rfc5987_then_legacy_url_and_mime() {
        let cases = [
            (
                Some(
                    "attachment; filename=plain.txt; filename*=UTF-8''%E0%A6%AC%E0%A6%BE%E0%A6%82%E0%A6%B2%E0%A6%BE.txt",
                ),
                "https://example.test/url.bin",
                Some("application/pdf"),
                "বাংলা.txt",
            ),
            (
                Some("attachment; filename=\"report; final.pdf\""),
                "https://example.test/url.bin",
                None,
                "report; final.pdf",
            ),
            (
                None,
                "https://example.test/files/hello%20world.txt?token=secret",
                None,
                "hello world.txt",
            ),
            (
                None,
                "https://example.test/",
                Some("image/png"),
                "download.png",
            ),
            (None, "https://example.test/", None, "download.bin"),
        ];
        for (disposition, value, mime, expected) in cases {
            assert_eq!(select_filename(disposition, &url(value), mime), expected);
        }
    }

    #[test]
    fn sanitizer_rejects_traversal_ads_devices_and_control_characters() {
        let cases = [
            (r"..\..\secret.txt", "______secret.txt"),
            ("report.txt:evil.exe", "report.txt_evil.exe"),
            ("CON.txt", "_CON.txt"),
            ("lpt9", "_lpt9"),
            ("bad\u{0000}name?.txt. ", "bad_name_.txt"),
            ("বাংলা ফাইল.txt", "বাংলা ফাইল.txt"),
        ];
        for (input, expected) in cases {
            assert_eq!(sanitize_windows_filename(input), expected, "{input:?}");
        }
        assert_eq!(
            safe_destination(Path::new(r"C:\Downloads"), r"..\NUL"),
            Path::new(r"C:\Downloads\___NUL")
        );
    }

    #[test]
    fn sanitizer_bounds_long_unicode_names_without_splitting_code_points() {
        let value = "文".repeat(500);
        let sanitized = sanitize_windows_filename(&value);
        assert_eq!(sanitized.chars().count(), 240);
        assert!(sanitized.is_char_boundary(sanitized.len()));
    }
}
