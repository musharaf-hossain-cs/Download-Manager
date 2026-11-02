use std::{cell::Cell, io::Cursor, path::Path, rc::Rc};

use download_manager_engine::{
    domain::HttpUrl,
    filename::{safe_destination, sanitize_windows_filename, select_filename},
    http_range::{ContentRange, parse_content_range},
    ipc::{
        ExtensionResponse, MAX_EXTENSION_MESSAGE_BYTES, RequestIdTracker, decode_extension_request,
    },
};
use download_manager_native_host::run_native_messaging_session;
use proptest::prelude::*;
use reqwest::header::{HeaderName, HeaderValue};

fn message_bytes() -> impl Strategy<Value = Vec<u8>> {
    prop_oneof![
        8 => prop::collection::vec(any::<u8>(), 0..=4_096),
        1 => prop::collection::vec(
            any::<u8>(),
            (MAX_EXTENSION_MESSAGE_BYTES - 16)..=(MAX_EXTENSION_MESSAGE_BYTES + 16),
        ),
    ]
}

fn header_bytes(maximum: usize) -> impl Strategy<Value = Vec<u8>> {
    prop::collection::vec(any::<u8>(), 0..=maximum)
}

proptest! {
    #[test]
    fn native_message_frames_never_panic_or_bypass_validation(
        body in message_bytes(),
        trailing in prop::collection::vec(any::<u8>(), 0..=32),
    ) {
        let accepted_directly = decode_extension_request(
            &body,
            &mut RequestIdTracker::default(),
        ).is_ok();
        let mut framed = u32::try_from(body.len())
            .expect("bounded generated body")
            .to_le_bytes()
            .to_vec();
        framed.extend_from_slice(&body);
        framed.extend_from_slice(&trailing);

        let calls = Rc::new(Cell::new(0_u8));
        let calls_in_handler = Rc::clone(&calls);
        let mut output = Vec::new();
        let result = run_native_messaging_session(Cursor::new(framed), &mut output, move |_| {
            calls_in_handler.set(calls_in_handler.get().saturating_add(1));
            ExtensionResponse::Accepted { job_ids: Vec::new() }
        });

        prop_assert_eq!(calls.get() == 1, accepted_directly);
        prop_assert!(calls.get() <= 1);
        if accepted_directly {
            prop_assert!(output.len() >= 4);
            let response_length = u32::from_le_bytes(
                output[..4].try_into().expect("checked response header"),
            ) as usize;
            prop_assert!(response_length <= MAX_EXTENSION_MESSAGE_BYTES);
            prop_assert!(output.len() >= response_length + 4);
        } else {
            prop_assert!(result.is_err());
            prop_assert!(output.is_empty());
        }
    }

    #[test]
    fn http_headers_and_content_ranges_remain_strict_and_canonical(
        name in header_bytes(128),
        value in header_bytes(2_048),
    ) {
        let _ = HeaderName::from_bytes(&name);
        let Ok(header_value) = HeaderValue::from_bytes(&value) else {
            return Ok(());
        };
        let Ok(text) = header_value.to_str() else {
            return Ok(());
        };
        let Some(parsed) = parse_content_range(text) else {
            return Ok(());
        };

        let canonical = match parsed {
            ContentRange::Satisfied {
                start,
                end,
                complete_length,
            } => {
                prop_assert!(start <= end);
                if let Some(length) = complete_length {
                    prop_assert!(end < length);
                }
                format!(
                    "bytes {start}-{end}/{}",
                    complete_length.map_or_else(|| "*".to_owned(), |length| length.to_string()),
                )
            }
            ContentRange::Unsatisfied { complete_length } => {
                format!("bytes */{complete_length}")
            }
        };
        prop_assert_eq!(parse_content_range(&canonical), Some(parsed));
    }

    #[test]
    fn filenames_always_collapse_to_one_bounded_windows_component(
        untrusted in any::<String>(),
        disposition in proptest::option::of(any::<String>()),
        mime_type in proptest::option::of(any::<String>()),
    ) {
        let selected = select_filename(
            disposition.as_deref(),
            &HttpUrl::parse("https://example.test/files/fallback.bin")
                .expect("static URL"),
            mime_type.as_deref(),
        );
        for candidate in [&untrusted, &selected] {
            let sanitized = sanitize_windows_filename(candidate);
            prop_assert!(!sanitized.is_empty());
            prop_assert!(sanitized.chars().count() <= 240);
            prop_assert!(!sanitized.ends_with(['.', ' ']));
            prop_assert!(!sanitized.contains(".."));
            let contains_invalid_character = sanitized.chars().any(|character| {
                character.is_control()
                    || matches!(character, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*')
            });
            prop_assert!(!contains_invalid_character);
            let destination = safe_destination(Path::new("fuzz-root"), candidate);
            prop_assert_eq!(destination.parent(), Some(Path::new("fuzz-root")));
            prop_assert_eq!(
                destination.file_name().and_then(|name| name.to_str()),
                Some(sanitized.as_str()),
            );
        }
    }
}
