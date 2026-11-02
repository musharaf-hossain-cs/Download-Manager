//! Strict parsing for HTTP `Content-Range` response headers.

/// A parsed byte `Content-Range` value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContentRange {
    /// A response represents the inclusive byte interval.
    Satisfied {
        start: u64,
        end: u64,
        complete_length: Option<u64>,
    },
    /// A response cannot satisfy a range for an object of the given length.
    Unsatisfied { complete_length: u64 },
}

impl ContentRange {
    /// Returns the complete representation length when the header provides it.
    #[must_use]
    pub const fn complete_length(self) -> Option<u64> {
        match self {
            Self::Satisfied {
                complete_length, ..
            } => complete_length,
            Self::Unsatisfied { complete_length } => Some(complete_length),
        }
    }

    /// Reports whether this is a satisfied range with the exact inclusive bounds.
    #[must_use]
    pub const fn matches(self, expected_start: u64, expected_end: u64) -> bool {
        matches!(
            self,
            Self::Satisfied { start, end, .. }
                if start == expected_start && end == expected_end
        )
    }

    /// Reports whether this range starts at `offset` and agrees with an optional known length.
    #[must_use]
    pub fn matches_resume(self, offset: u64, expected_length: Option<u64>) -> bool {
        match self {
            Self::Satisfied {
                start,
                complete_length,
                ..
            } => {
                start == offset
                    && match expected_length {
                        Some(expected) => complete_length == Some(expected),
                        None => true,
                    }
            }
            Self::Unsatisfied { .. } => false,
        }
    }
}

/// Parses the RFC byte-range forms `bytes START-END/TOTAL`, `bytes START-END/*`, and
/// `bytes */TOTAL` without accepting whitespace, overflow, inverted bounds, or an impossible
/// complete length.
#[must_use]
pub fn parse_content_range(value: &str) -> Option<ContentRange> {
    let value = value.strip_prefix("bytes ")?;
    let (bounds, represented_total) = value.split_once('/')?;
    if represented_total.contains('/') {
        return None;
    }
    let complete_length = if represented_total == "*" {
        None
    } else {
        Some(represented_total.parse::<u64>().ok()?)
    };

    if bounds == "*" {
        return Some(ContentRange::Unsatisfied {
            complete_length: complete_length?,
        });
    }

    let (start, end) = bounds.split_once('-')?;
    if end.contains('-') {
        return None;
    }
    let start = start.parse::<u64>().ok()?;
    let end = end.parse::<u64>().ok()?;
    if start > end || complete_length.is_some_and(|length| end >= length) {
        return None;
    }
    Some(ContentRange::Satisfied {
        start,
        end,
        complete_length,
    })
}

#[cfg(test)]
mod tests {
    use super::{ContentRange, parse_content_range};

    #[test]
    fn parses_only_structurally_valid_byte_content_ranges() {
        assert_eq!(
            parse_content_range("bytes 5-10/11"),
            Some(ContentRange::Satisfied {
                start: 5,
                end: 10,
                complete_length: Some(11),
            })
        );
        assert_eq!(
            parse_content_range("bytes 5-10/*"),
            Some(ContentRange::Satisfied {
                start: 5,
                end: 10,
                complete_length: None,
            })
        );
        assert_eq!(
            parse_content_range("bytes */0"),
            Some(ContentRange::Unsatisfied { complete_length: 0 })
        );
        for invalid in [
            "bytes 10-5/11",
            "bytes 0-10/10",
            "bytes 0-0/*/1",
            "bytes 0--1/2",
            "Bytes 0-0/1",
            "bytes */*",
            " bytes 0-0/1",
        ] {
            assert_eq!(parse_content_range(invalid), None, "{invalid:?}");
        }
    }
}
