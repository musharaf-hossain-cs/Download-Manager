//! Central, deterministic secret redaction for persisted diagnostics.

use percent_encoding::percent_decode_str;
use serde_json::{Map, Value};

const REDACTED: &str = "[REDACTED]";

/// Removes recognized credentials, authentication material, and signed-URL secrets from text.
///
/// The transformation is deterministic and idempotent. It deliberately preserves non-secret URL
/// components and fields so diagnostics remain useful without retaining reusable credentials.
#[must_use]
pub fn redact_secrets(input: &str) -> String {
    let headers = redact_header_lines(input);
    let assignments = redact_sensitive_assignments(&headers);
    redact_urls(&assignments)
}

pub(crate) fn redact_json_value(value: &Value) -> Value {
    match value {
        Value::Array(values) => Value::Array(values.iter().map(redact_json_value).collect()),
        Value::Object(values) => Value::Object(
            values
                .iter()
                .map(|(key, value)| {
                    let value = if is_sensitive_field(key) {
                        Value::String(REDACTED.to_owned())
                    } else {
                        redact_json_value(value)
                    };
                    (redact_secrets(key), value)
                })
                .collect::<Map<_, _>>(),
        ),
        Value::String(value) => Value::String(redact_secrets(value)),
        value => value.clone(),
    }
}

pub(crate) fn is_sensitive_query_parameter(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    name.starts_with("x-amz-")
        || name.starts_with("x-goog-")
        || matches!(
            name.as_str(),
            "token"
                | "auth"
                | "authorization"
                | "sig"
                | "signature"
                | "expires"
                | "expiry"
                | "exp"
                | "policy"
                | "key-pair-id"
                | "hdnts"
        )
}

fn redact_header_lines(input: &str) -> String {
    let mut output = String::with_capacity(input.len());
    for line in input.split_inclusive('\n') {
        let (content, ending) = if let Some(content) = line.strip_suffix("\r\n") {
            (content, "\r\n")
        } else if let Some(content) = line.strip_suffix('\n') {
            (content, "\n")
        } else {
            (line, "")
        };
        let leading = content.len() - content.trim_start_matches([' ', '\t']).len();
        let Some(relative_colon) = content[leading..].find(':') else {
            output.push_str(line);
            continue;
        };
        let colon = leading + relative_colon;
        if !is_sensitive_header(&content[leading..colon]) {
            output.push_str(line);
            continue;
        }
        output.push_str(&content[..=colon]);
        if content[colon + 1..].starts_with([' ', '\t']) {
            output.push(' ');
        }
        output.push_str(REDACTED);
        output.push_str(ending);
    }
    output
}

fn redact_sensitive_assignments(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut output = Vec::with_capacity(bytes.len());
    let mut copied_through = 0;
    let mut index = 0;
    while index < bytes.len() {
        if !is_key_byte(bytes[index]) || index > 0 && is_key_byte(bytes[index - 1]) {
            index += 1;
            continue;
        }
        let key_start = index;
        while index < bytes.len() && is_key_byte(bytes[index]) {
            index += 1;
        }
        let key_end = index;
        let quote = key_start
            .checked_sub(1)
            .and_then(|before| matches!(bytes[before], b'\'' | b'"').then_some(bytes[before]));
        if quote.is_some_and(|quote| bytes.get(index) == Some(&quote)) {
            index += 1;
        }
        while bytes.get(index).is_some_and(u8::is_ascii_whitespace) {
            index += 1;
        }
        let key = &input[key_start..key_end];
        if !matches!(bytes.get(index), Some(b'=' | b':')) || !is_sensitive_field(key) {
            index = key_end.max(key_start + 1);
            continue;
        }
        index += 1;
        while bytes.get(index).is_some_and(u8::is_ascii_whitespace) {
            index += 1;
        }
        if index >= bytes.len() {
            break;
        }

        let (value_start, value_end) = if matches!(bytes[index], b'\'' | b'"') {
            let value_quote = bytes[index];
            let start = index + 1;
            index = start;
            let mut escaped = false;
            while index < bytes.len() {
                if !escaped && bytes[index] == value_quote {
                    break;
                }
                escaped = !escaped && bytes[index] == b'\\';
                if bytes[index] != b'\\' {
                    escaped = false;
                }
                index += 1;
            }
            (start, index)
        } else {
            let start = index;
            while index < bytes.len() && !is_assignment_terminator(bytes[index], key) {
                index += 1;
            }
            (start, index)
        };
        if value_start == value_end {
            continue;
        }
        output.extend_from_slice(&bytes[copied_through..value_start]);
        output.extend_from_slice(REDACTED.as_bytes());
        copied_through = value_end;
    }
    output.extend_from_slice(&bytes[copied_through..]);
    String::from_utf8(output).expect("redaction preserves valid UTF-8")
}

fn redact_urls(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut output = String::with_capacity(input.len());
    let mut copied_through = 0;
    let mut index = 0;
    while index < bytes.len() {
        let scheme_length = if starts_ascii_case_insensitive(&bytes[index..], b"https://") {
            8
        } else if starts_ascii_case_insensitive(&bytes[index..], b"http://") {
            7
        } else {
            index += 1;
            continue;
        };
        if index > 0
            && matches!(bytes[index - 1], b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'+' | b'-' | b'.')
        {
            index += scheme_length;
            continue;
        }
        let mut end = index + scheme_length;
        while end < bytes.len() && !is_url_terminator(bytes[end]) {
            end += 1;
        }
        let candidate = &input[index..end];
        let sanitized = sanitize_url(candidate, scheme_length);
        if sanitized != candidate {
            output.push_str(&input[copied_through..index]);
            output.push_str(&sanitized);
            copied_through = end;
        }
        index = end.max(index + 1);
    }
    output.push_str(&input[copied_through..]);
    output
}

fn sanitize_url(url: &str, scheme_length: usize) -> String {
    let bytes = url.as_bytes();
    let authority_end = bytes[scheme_length..]
        .iter()
        .position(|byte| matches!(byte, b'/' | b'?' | b'#'))
        .map_or(bytes.len(), |offset| scheme_length + offset);
    let userinfo = bytes[scheme_length..authority_end]
        .iter()
        .rposition(|byte| *byte == b'@')
        .map(|offset| (scheme_length, scheme_length + offset));

    let query_start = bytes
        .iter()
        .position(|byte| *byte == b'?')
        .map(|position| position + 1);
    let query_end = bytes
        .iter()
        .position(|byte| *byte == b'#')
        .unwrap_or(bytes.len());
    let mut replacements = Vec::new();
    if let Some(userinfo) = userinfo {
        replacements.push(userinfo);
    }
    if let Some(query_start) = query_start.filter(|start| *start <= query_end) {
        let mut pair_start = query_start;
        while pair_start <= query_end {
            let pair_end = bytes[pair_start..query_end]
                .iter()
                .position(|byte| *byte == b'&')
                .map_or(query_end, |offset| pair_start + offset);
            if let Some(equals) = bytes[pair_start..pair_end]
                .iter()
                .position(|byte| *byte == b'=')
                .map(|offset| pair_start + offset)
            {
                let name = percent_decode_str(&url[pair_start..equals]).decode_utf8_lossy();
                if is_sensitive_url_query_parameter(&name) && equals < pair_end {
                    replacements.push((equals + 1, pair_end));
                }
            }
            if pair_end == query_end {
                break;
            }
            pair_start = pair_end + 1;
        }
    }
    if replacements.is_empty() {
        return url.to_owned();
    }
    replacements.sort_unstable_by_key(|replacement| replacement.0);
    let mut output = String::with_capacity(url.len());
    let mut copied_through = 0;
    for (start, end) in replacements {
        output.push_str(&url[copied_through..start]);
        output.push_str(REDACTED);
        copied_through = end;
    }
    output.push_str(&url[copied_through..]);
    output
}

fn is_sensitive_header(name: &str) -> bool {
    let name = normalize_name(name);
    matches!(
        leaf_name(&name),
        "authorization" | "proxy-authorization" | "cookie" | "set-cookie"
    )
}

fn is_sensitive_field(name: &str) -> bool {
    let name = normalize_name(name);
    is_sensitive_normalized_field(leaf_name(&name))
}

fn is_sensitive_url_query_parameter(name: &str) -> bool {
    is_sensitive_query_parameter(name) || is_sensitive_field(name)
}

fn is_sensitive_normalized_field(name: &str) -> bool {
    matches!(
        name,
        "authorization"
            | "proxy-authorization"
            | "cookie"
            | "set-cookie"
            | "token"
            | "access-token"
            | "accesstoken"
            | "refresh-token"
            | "refreshtoken"
            | "id-token"
            | "idtoken"
            | "session-token"
            | "sessiontoken"
            | "auth"
            | "api-key"
            | "apikey"
            | "x-api-key"
            | "xapikey"
            | "password"
            | "passwd"
            | "proxy-password"
            | "proxypassword"
            | "secret"
            | "client-secret"
            | "clientsecret"
            | "session"
            | "session-id"
            | "sessionid"
            | "awsaccesskeyid"
            | "sig"
            | "signature"
    )
}

fn leaf_name(name: &str) -> &str {
    name.rsplit('.').next().unwrap_or(name)
}

fn normalize_name(name: &str) -> String {
    percent_decode_str(name.trim())
        .decode_utf8_lossy()
        .chars()
        .map(|character| {
            if character == '_' {
                '-'
            } else {
                character.to_ascii_lowercase()
            }
        })
        .collect()
}

fn is_key_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.' | b'%')
}

fn is_assignment_terminator(byte: u8, key: &str) -> bool {
    if is_sensitive_header(key) {
        matches!(byte, b'\r' | b'\n' | b'&' | b'}')
    } else {
        byte.is_ascii_whitespace() || matches!(byte, b'&' | b';' | b',' | b'}')
    }
}

fn starts_ascii_case_insensitive(value: &[u8], prefix: &[u8]) -> bool {
    value
        .get(..prefix.len())
        .is_some_and(|value| value.eq_ignore_ascii_case(prefix))
}

fn is_url_terminator(byte: u8) -> bool {
    byte.is_ascii_whitespace()
        || matches!(
            byte,
            b'\'' | b'"' | b'<' | b'>' | b'{' | b'}' | b'(' | b')' | b','
        )
}

#[cfg(test)]
mod tests {
    use super::redact_secrets;

    fn assert_absent(redacted: &str, secrets: &[&str]) {
        for secret in secrets {
            assert!(!redacted.contains(secret), "secret remained: {secret}");
        }
    }

    #[test]
    fn auth_cookie_and_token_corpus_never_emits_secret_values() {
        let input = "Authorization: Basic basic-sentinel\r\n\
                     proxy-authorization: Digest digest-sentinel\n\
                     Cookie: session=cookie-sentinel\n\
                     Set-Cookie: id=set-cookie-sentinel; Secure\n\
                     received Authorization: Bearer embedded-auth-sentinel with context\n\
                     {\"access_token\":\"json-token-sentinel\",\"accessToken\":\"camel-access-sentinel\",\"refreshToken\":\"camel-refresh-sentinel\",\"sessionToken\":\"camel-session-sentinel\",\"clientSecret\":\"client-secret-sentinel\",\"proxyPassword\":\"proxy-password-field-sentinel\",\"password\":\"password-sentinel\",\"x-api-key\":\"api-sentinel\",\"safe\":\"visible\"}\n\
                     request.authorization=Bearer dotted-auth-sentinel\n\
                     auth.access_token=dotted-token-sentinel\n\
                     token=raw-token-sentinel&ordinary=kept";
        let redacted = redact_secrets(input);

        assert_absent(
            &redacted,
            &[
                "basic-sentinel",
                "digest-sentinel",
                "cookie-sentinel",
                "set-cookie-sentinel",
                "embedded-auth-sentinel",
                "json-token-sentinel",
                "camel-access-sentinel",
                "camel-refresh-sentinel",
                "camel-session-sentinel",
                "client-secret-sentinel",
                "proxy-password-field-sentinel",
                "password-sentinel",
                "api-sentinel",
                "dotted-auth-sentinel",
                "dotted-token-sentinel",
                "raw-token-sentinel",
            ],
        );
        assert!(redacted.contains("visible"));
        assert!(redacted.contains("ordinary=kept"));
    }

    #[test]
    fn signed_url_and_proxy_credential_corpus_is_redacted() {
        let input = "https://url-user:url-password@files.test/archive?safe=visible&token=generic-sentinel \
                     http://proxy-user:proxy-password@proxy.test:8080/ \
                     https://s3.test/object?X-Amz-Signature=aws-sentinel&X-Amz-Date=20261029 \
                     https://storage.test/object?X%2DGoog%2DSignature=gcs-sentinel \
                     https://blob.test/object?sv=1&sig=azure-sentinel \
                     https://cloudfront.test/object?Policy=policy-sentinel&Signature=cloudfront-sentinel&Key-Pair-Id=pair-sentinel";
        let redacted = redact_secrets(input);

        assert_absent(
            &redacted,
            &[
                "url-user",
                "url-password",
                "proxy-user",
                "proxy-password",
                "generic-sentinel",
                "aws-sentinel",
                "gcs-sentinel",
                "azure-sentinel",
                "policy-sentinel",
                "cloudfront-sentinel",
                "pair-sentinel",
            ],
        );
        assert!(redacted.contains("safe=visible"));
        assert!(redacted.contains("sv=1"));
    }

    #[test]
    fn redaction_is_case_insensitive_idempotent_and_boundary_safe() {
        let safe = "token_count=17 authorization_status=ok password_policy=strict policy=allow expires=123 https://example.test/file?q=বাংলা";
        assert_eq!(redact_secrets(safe), safe);

        let input = "AUTHORIZATION: Bearer mixed-case-sentinel\naccess%5Ftoken=encoded-sentinel\nmalformed https://user:malformed-sentinel@";
        let once = redact_secrets(input);
        assert_absent(
            &once,
            &[
                "mixed-case-sentinel",
                "encoded-sentinel",
                "malformed-sentinel",
            ],
        );
        assert_eq!(redact_secrets(&once), once);
    }
}
