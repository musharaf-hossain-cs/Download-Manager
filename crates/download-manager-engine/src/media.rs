//! Standards-focused media manifest parsing.

use std::collections::BTreeMap;

use thiserror::Error;
use url::Url;

use crate::domain::HttpUrl;

/// A parsed HLS multivariant (master) playlist.
#[derive(Debug, Clone, PartialEq)]
pub struct HlsMasterPlaylist {
    pub variants: Vec<HlsVariant>,
    pub audio_renditions: Vec<HlsRendition>,
    pub subtitle_renditions: Vec<HlsRendition>,
}

/// One selectable stream declared by `EXT-X-STREAM-INF`.
#[derive(Debug, Clone, PartialEq)]
pub struct HlsVariant {
    pub uri: HttpUrl,
    pub bandwidth: u64,
    pub resolution: Option<(u32, u32)>,
    pub frame_rate: Option<f64>,
    pub codecs: Vec<String>,
    pub audio_group: Option<String>,
    pub subtitle_group: Option<String>,
}

/// One external audio or subtitle rendition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HlsRendition {
    pub group_id: String,
    pub name: String,
    pub language: Option<String>,
    pub uri: Option<HttpUrl>,
    pub is_default: bool,
}

/// A parsed HLS media playlist in playback order.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct HlsMediaPlaylist {
    pub media_sequence: u64,
    pub target_duration: u64,
    pub segments: Vec<HlsMediaSegment>,
    pub has_end_list: bool,
}

/// One media fragment and the state that applies when it begins.
#[derive(Debug, Clone, PartialEq)]
pub struct HlsMediaSegment {
    pub sequence: u64,
    pub uri: HttpUrl,
    pub duration_seconds: f64,
    pub byte_range: Option<HlsByteRange>,
    pub init_segment: Option<HlsInitSegment>,
    pub discontinuity: bool,
}

/// An inclusive byte window expressed as an offset and non-zero length.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HlsByteRange {
    pub offset: u64,
    pub length: u64,
}

/// The fMP4 initialization object active for a media fragment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HlsInitSegment {
    pub uri: HttpUrl,
    pub byte_range: Option<HlsByteRange>,
}

/// Coarse, non-secret classification for media that this engine must not download.
///
/// The classifier never exposes key URIs, key IDs, PSSH payloads, or license responses. Its only
/// purpose is to produce a stable user-facing refusal before any track download or worker step.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProtectedMediaKind {
    HlsEncryption,
    CommonEncryption,
    Widevine,
    PlayReady,
    FairPlay,
    Unknown,
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum HlsMediaError {
    #[error("playlist must begin with #EXTM3U")]
    MissingHeader,
    #[error("playlist does not contain media segments")]
    NoSegments,
    #[error("master-playlist tags are not valid in a media playlist")]
    MasterPlaylist,
    #[error("malformed {0} tag")]
    MalformedTag(&'static str),
    #[error("{tag} is missing required attribute {attribute}")]
    MissingAttribute {
        tag: &'static str,
        attribute: &'static str,
    },
    #[error("invalid {attribute} attribute")]
    InvalidAttribute { attribute: &'static str },
    #[error("invalid or unsupported media URI")]
    InvalidUri,
    #[error("byte-range offset is missing without a preceding range for the same URI")]
    MissingByteRangeOffset,
    #[error("playlist sequence or byte range overflows")]
    Overflow,
    #[error("protected/unsupported media ({0:?})")]
    ProtectedMedia(ProtectedMediaKind),
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum HlsMasterError {
    #[error("playlist must begin with #EXTM3U")]
    MissingHeader,
    #[error("playlist does not contain HLS master tags")]
    NotMasterPlaylist,
    #[error("malformed {0} tag")]
    MalformedTag(&'static str),
    #[error("{tag} is missing required attribute {attribute}")]
    MissingAttribute {
        tag: &'static str,
        attribute: &'static str,
    },
    #[error("invalid {attribute} attribute")]
    InvalidAttribute { attribute: &'static str },
    #[error("invalid or unsupported media URI")]
    InvalidUri,
    #[error("protected/unsupported media ({0:?})")]
    ProtectedMedia(ProtectedMediaKind),
}

/// Parses an HLS master playlist and resolves every media URI against its URL.
///
/// Media-playlist segments are intentionally outside this contract.
///
/// # Errors
/// Returns an error for protected media, a non-master playlist, or malformed required metadata/URI.
pub fn parse_hls_master_playlist(
    manifest: &str,
    manifest_url: &HttpUrl,
) -> Result<HlsMasterPlaylist, HlsMasterError> {
    let mut lines = manifest
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty());
    let header = lines.next().map(|line| line.trim_start_matches('\u{feff}'));
    if header != Some("#EXTM3U") {
        return Err(HlsMasterError::MissingHeader);
    }

    let base = Url::parse(manifest_url.as_str()).map_err(|_| HlsMasterError::InvalidUri)?;
    let remaining: Vec<_> = lines.collect();
    if let Some(kind) = classify_hls_protection(&remaining) {
        return Err(HlsMasterError::ProtectedMedia(kind));
    }
    let mut playlist = HlsMasterPlaylist {
        variants: Vec::new(),
        audio_renditions: Vec::new(),
        subtitle_renditions: Vec::new(),
    };
    let mut index = 0;
    while index < remaining.len() {
        let line = remaining[index];
        if let Some(attributes) = line.strip_prefix("#EXT-X-STREAM-INF:") {
            let attributes = parse_attributes(attributes, "EXT-X-STREAM-INF")?;
            let uri = remaining
                .get(index + 1)
                .filter(|uri| !uri.starts_with('#'))
                .ok_or(HlsMasterError::MalformedTag("EXT-X-STREAM-INF"))?;
            playlist
                .variants
                .push(parse_variant(&base, &attributes, uri)?);
            index += 2;
            continue;
        }
        if let Some(attributes) = line.strip_prefix("#EXT-X-MEDIA:") {
            let attributes = parse_attributes(attributes, "EXT-X-MEDIA")?;
            let rendition_type = required(&attributes, "TYPE", "EXT-X-MEDIA")?;
            if matches!(rendition_type, "AUDIO" | "SUBTITLES") {
                let rendition = parse_rendition(&base, &attributes, rendition_type)?;
                if rendition_type == "AUDIO" {
                    playlist.audio_renditions.push(rendition);
                } else {
                    playlist.subtitle_renditions.push(rendition);
                }
            }
        }
        index += 1;
    }
    if playlist.variants.is_empty()
        && playlist.audio_renditions.is_empty()
        && playlist.subtitle_renditions.is_empty()
    {
        return Err(HlsMasterError::NotMasterPlaylist);
    }
    Ok(playlist)
}

/// Parses an HLS media playlist, resolving ordered TS/fMP4 fragments and active init sections.
///
/// Unknown extension tags are retained only through their effect on ordering: this slice models
/// byte ranges and discontinuities, while encryption and live-window policy belong to later
/// contracts.
///
/// # Errors
/// Returns an error for protected media, a master playlist, missing/malformed segment metadata,
/// invalid byte-range chaining, arithmetic overflow, or non-HTTP(S) URIs.
pub fn parse_hls_media_playlist(
    manifest: &str,
    manifest_url: &HttpUrl,
) -> Result<HlsMediaPlaylist, HlsMediaError> {
    let mut lines = manifest
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty());
    let header = lines.next().map(|line| line.trim_start_matches('\u{feff}'));
    if header != Some("#EXTM3U") {
        return Err(HlsMediaError::MissingHeader);
    }

    let base = Url::parse(manifest_url.as_str()).map_err(|_| HlsMediaError::InvalidUri)?;
    let remaining = lines.collect::<Vec<_>>();
    if let Some(kind) = classify_hls_protection(&remaining) {
        return Err(HlsMediaError::ProtectedMedia(kind));
    }
    let mut state = MediaParseState::default();
    for line in remaining {
        state.parse_line(&base, line)?;
    }
    state.finish()
}

fn classify_hls_protection(lines: &[&str]) -> Option<ProtectedMediaKind> {
    lines.iter().find_map(|line| {
        let (is_session_key, attributes) = line
            .strip_prefix("#EXT-X-SESSION-KEY:")
            .map(|attributes| (true, attributes))
            .or_else(|| {
                line.strip_prefix("#EXT-X-KEY:")
                    .map(|attributes| (false, attributes))
            })?;
        let parsed = parse_attribute_entries(attributes);
        if !is_session_key
            && parsed
                .as_ref()
                .and_then(|values| values.get("METHOD"))
                .is_some_and(|method| method.eq_ignore_ascii_case("NONE"))
        {
            return None;
        }
        Some(classify_hls_key_format(parsed.as_ref()))
    })
}

fn classify_hls_key_format(attributes: Option<&BTreeMap<String, String>>) -> ProtectedMediaKind {
    let key_format = attributes
        .and_then(|values| values.get("KEYFORMAT"))
        .map(|value| value.to_ascii_lowercase());
    match key_format.as_deref() {
        Some(value) if value.contains("edef8ba9") || value.contains("widevine") => {
            ProtectedMediaKind::Widevine
        }
        Some(value) if value.contains("9a04f079") || value.contains("playready") => {
            ProtectedMediaKind::PlayReady
        }
        Some(value) if value.contains("94ce86fb") || value.contains("streamingkeydelivery") => {
            ProtectedMediaKind::FairPlay
        }
        Some("identity") | None => ProtectedMediaKind::HlsEncryption,
        Some(_) => ProtectedMediaKind::Unknown,
    }
}

#[derive(Default)]
struct MediaParseState {
    playlist: HlsMediaPlaylist,
    pending_duration: Option<f64>,
    pending_range: Option<RawByteRange>,
    pending_discontinuity: bool,
    active_init: Option<HlsInitSegment>,
    previous_range: Option<(HttpUrl, HlsByteRange)>,
    previous_map_range: Option<(HttpUrl, HlsByteRange)>,
}

impl MediaParseState {
    fn parse_line(&mut self, base: &Url, line: &str) -> Result<(), HlsMediaError> {
        if line.starts_with("#EXT-X-STREAM-INF:") || line.starts_with("#EXT-X-I-FRAME-STREAM-INF:")
        {
            return Err(HlsMediaError::MasterPlaylist);
        }
        if let Some(value) = line.strip_prefix("#EXT-X-MEDIA-SEQUENCE:") {
            if !self.playlist.segments.is_empty() || self.playlist.media_sequence != 0 {
                return Err(HlsMediaError::MalformedTag("EXT-X-MEDIA-SEQUENCE"));
            }
            self.playlist.media_sequence = parse_u64(value, "MEDIA-SEQUENCE", true)?;
            return Ok(());
        }
        if let Some(value) = line.strip_prefix("#EXT-X-TARGETDURATION:") {
            if self.playlist.target_duration != 0 {
                return Err(HlsMediaError::MalformedTag("EXT-X-TARGETDURATION"));
            }
            self.playlist.target_duration = parse_u64(value, "TARGETDURATION", false)?;
            return Ok(());
        }
        if let Some(value) = line.strip_prefix("#EXTINF:") {
            if self.pending_duration.is_some() {
                return Err(HlsMediaError::MalformedTag("EXTINF"));
            }
            let duration = value
                .split_once(',')
                .map_or(value, |(duration, _)| duration);
            self.pending_duration = Some(parse_media_duration(duration)?);
            return Ok(());
        }
        if let Some(value) = line.strip_prefix("#EXT-X-BYTERANGE:") {
            if self.pending_range.is_some() {
                return Err(HlsMediaError::MalformedTag("EXT-X-BYTERANGE"));
            }
            self.pending_range = Some(parse_raw_byte_range(value)?);
            return Ok(());
        }
        if let Some(value) = line.strip_prefix("#EXT-X-MAP:") {
            self.parse_map(base, value)?;
            return Ok(());
        }
        if line == "#EXT-X-DISCONTINUITY" {
            if self.pending_discontinuity {
                return Err(HlsMediaError::MalformedTag("EXT-X-DISCONTINUITY"));
            }
            self.pending_discontinuity = true;
            return Ok(());
        }
        if line == "#EXT-X-ENDLIST" {
            self.playlist.has_end_list = true;
            return Ok(());
        }
        if line.starts_with('#') {
            return Ok(());
        }
        self.push_segment(base, line)
    }

    fn parse_map(&mut self, base: &Url, value: &str) -> Result<(), HlsMediaError> {
        let attributes = parse_media_attributes(value, "EXT-X-MAP")?;
        let uri = attributes
            .get("URI")
            .ok_or(HlsMediaError::MissingAttribute {
                tag: "EXT-X-MAP",
                attribute: "URI",
            })
            .and_then(|value| resolve_media_uri(base, value))?;
        let byte_range = attributes
            .get("BYTERANGE")
            .map(|value| parse_raw_byte_range(value))
            .transpose()?
            .map(|range| resolve_byte_range(range, &uri, self.previous_map_range.as_ref()))
            .transpose()?;
        if let Some(range) = byte_range {
            self.previous_map_range = Some((uri.clone(), range));
        }
        self.active_init = Some(HlsInitSegment { uri, byte_range });
        Ok(())
    }

    fn push_segment(&mut self, base: &Url, line: &str) -> Result<(), HlsMediaError> {
        if self.playlist.has_end_list {
            return Err(HlsMediaError::MalformedTag("EXT-X-ENDLIST"));
        }
        let duration_seconds = self
            .pending_duration
            .take()
            .ok_or(HlsMediaError::MalformedTag("EXTINF"))?;
        let uri = resolve_media_uri(base, line)?;
        let byte_range = self
            .pending_range
            .take()
            .map(|range| resolve_byte_range(range, &uri, self.previous_range.as_ref()))
            .transpose()?;
        if let Some(range) = byte_range {
            self.previous_range = Some((uri.clone(), range));
        } else {
            self.previous_range = None;
        }
        let index =
            u64::try_from(self.playlist.segments.len()).map_err(|_| HlsMediaError::Overflow)?;
        let sequence = self
            .playlist
            .media_sequence
            .checked_add(index)
            .ok_or(HlsMediaError::Overflow)?;
        self.playlist.segments.push(HlsMediaSegment {
            sequence,
            uri,
            duration_seconds,
            byte_range,
            init_segment: self.active_init.clone(),
            discontinuity: std::mem::take(&mut self.pending_discontinuity),
        });
        Ok(())
    }

    fn finish(self) -> Result<HlsMediaPlaylist, HlsMediaError> {
        if self.pending_duration.is_some()
            || self.pending_range.is_some()
            || self.pending_discontinuity
        {
            return Err(HlsMediaError::MalformedTag("segment"));
        }
        if self.playlist.segments.is_empty() {
            return Err(HlsMediaError::NoSegments);
        }
        if self.playlist.target_duration == 0 {
            return Err(HlsMediaError::MissingAttribute {
                tag: "playlist",
                attribute: "EXT-X-TARGETDURATION",
            });
        }
        Ok(self.playlist)
    }
}

#[derive(Debug, Clone, Copy)]
struct RawByteRange {
    length: u64,
    offset: Option<u64>,
}

fn parse_raw_byte_range(value: &str) -> Result<RawByteRange, HlsMediaError> {
    let (length, offset) = value
        .trim()
        .split_once('@')
        .map_or((value.trim(), None), |parts| {
            (parts.0.trim(), Some(parts.1.trim()))
        });
    let length = parse_u64(length, "BYTERANGE", false)?;
    let offset = offset
        .map(|value| parse_u64(value, "BYTERANGE", true))
        .transpose()?;
    Ok(RawByteRange { length, offset })
}

fn resolve_byte_range(
    raw: RawByteRange,
    uri: &HttpUrl,
    previous: Option<&(HttpUrl, HlsByteRange)>,
) -> Result<HlsByteRange, HlsMediaError> {
    let offset = if let Some(offset) = raw.offset {
        offset
    } else {
        let (previous_uri, previous_range) =
            previous.ok_or(HlsMediaError::MissingByteRangeOffset)?;
        if previous_uri != uri {
            return Err(HlsMediaError::MissingByteRangeOffset);
        }
        previous_range
            .offset
            .checked_add(previous_range.length)
            .ok_or(HlsMediaError::Overflow)?
    };
    offset
        .checked_add(raw.length)
        .ok_or(HlsMediaError::Overflow)?;
    Ok(HlsByteRange {
        offset,
        length: raw.length,
    })
}

fn parse_media_duration(value: &str) -> Result<f64, HlsMediaError> {
    value
        .trim()
        .parse::<f64>()
        .ok()
        .filter(|value| value.is_finite() && *value > 0.0)
        .ok_or(HlsMediaError::InvalidAttribute {
            attribute: "EXTINF duration",
        })
}

fn parse_u64(value: &str, attribute: &'static str, allow_zero: bool) -> Result<u64, HlsMediaError> {
    value
        .trim()
        .parse::<u64>()
        .ok()
        .filter(|value| allow_zero || *value > 0)
        .ok_or(HlsMediaError::InvalidAttribute { attribute })
}

fn parse_media_attributes(
    input: &str,
    tag: &'static str,
) -> Result<BTreeMap<String, String>, HlsMediaError> {
    parse_attribute_entries(input).ok_or(HlsMediaError::MalformedTag(tag))
}

fn parse_attribute_entries(input: &str) -> Option<BTreeMap<String, String>> {
    let mut values = BTreeMap::new();
    let mut quoted = false;
    let mut start = 0;
    for (index, character) in input
        .char_indices()
        .chain(std::iter::once((input.len(), ',')))
    {
        if character == '"' {
            quoted = !quoted;
        }
        if character == ',' && !quoted {
            let entry = input.get(start..index)?.trim();
            let (name, raw) = entry.split_once('=')?;
            let value = match (raw.strip_prefix('"'), raw.strip_suffix('"')) {
                (Some(without_start), Some(_)) => without_start.strip_suffix('"')?,
                (None, None) => raw,
                _ => return None,
            };
            if name.is_empty()
                || value.is_empty()
                || values.insert(name.to_owned(), value.to_owned()).is_some()
            {
                return None;
            }
            start = index + 1;
        }
    }
    (!quoted && !values.is_empty()).then_some(values)
}

fn resolve_media_uri(base: &Url, uri: &str) -> Result<HttpUrl, HlsMediaError> {
    let resolved = base.join(uri).map_err(|_| HlsMediaError::InvalidUri)?;
    HttpUrl::parse(resolved.to_string()).map_err(|_| HlsMediaError::InvalidUri)
}

fn parse_variant(
    base: &Url,
    attributes: &BTreeMap<String, String>,
    uri: &str,
) -> Result<HlsVariant, HlsMasterError> {
    let bandwidth = required(attributes, "BANDWIDTH", "EXT-X-STREAM-INF")?
        .parse::<u64>()
        .ok()
        .filter(|value| *value > 0)
        .ok_or(HlsMasterError::InvalidAttribute {
            attribute: "BANDWIDTH",
        })?;
    let resolution = attributes
        .get("RESOLUTION")
        .map(|value| parse_resolution(value))
        .transpose()?;
    let frame_rate = attributes
        .get("FRAME-RATE")
        .map(|value| parse_positive_f64(value, "FRAME-RATE"))
        .transpose()?;
    let codecs = attributes
        .get("CODECS")
        .map(|value| value.split(',').map(str::trim).map(str::to_owned).collect())
        .unwrap_or_default();
    Ok(HlsVariant {
        uri: resolve_uri(base, uri)?,
        bandwidth,
        resolution,
        frame_rate,
        codecs,
        audio_group: attributes.get("AUDIO").cloned(),
        subtitle_group: attributes.get("SUBTITLES").cloned(),
    })
}

fn parse_rendition(
    base: &Url,
    attributes: &BTreeMap<String, String>,
    rendition_type: &str,
) -> Result<HlsRendition, HlsMasterError> {
    let uri = attributes
        .get("URI")
        .map(|value| resolve_uri(base, value))
        .transpose()?;
    if rendition_type == "SUBTITLES" && uri.is_none() {
        return Err(HlsMasterError::MissingAttribute {
            tag: "EXT-X-MEDIA",
            attribute: "URI",
        });
    }
    Ok(HlsRendition {
        group_id: required(attributes, "GROUP-ID", "EXT-X-MEDIA")?.to_owned(),
        name: required(attributes, "NAME", "EXT-X-MEDIA")?.to_owned(),
        language: attributes.get("LANGUAGE").cloned(),
        uri,
        is_default: attributes
            .get("DEFAULT")
            .is_some_and(|value| value == "YES"),
    })
}

fn parse_attributes(
    input: &str,
    tag: &'static str,
) -> Result<BTreeMap<String, String>, HlsMasterError> {
    parse_attribute_entries(input).ok_or(HlsMasterError::MalformedTag(tag))
}

fn required<'a>(
    attributes: &'a BTreeMap<String, String>,
    attribute: &'static str,
    tag: &'static str,
) -> Result<&'a str, HlsMasterError> {
    attributes
        .get(attribute)
        .map(String::as_str)
        .ok_or(HlsMasterError::MissingAttribute { tag, attribute })
}

fn resolve_uri(base: &Url, uri: &str) -> Result<HttpUrl, HlsMasterError> {
    let resolved = base.join(uri).map_err(|_| HlsMasterError::InvalidUri)?;
    HttpUrl::parse(resolved.to_string()).map_err(|_| HlsMasterError::InvalidUri)
}

fn parse_resolution(value: &str) -> Result<(u32, u32), HlsMasterError> {
    let (width, height) = value
        .split_once('x')
        .ok_or(HlsMasterError::InvalidAttribute {
            attribute: "RESOLUTION",
        })?;
    let width = width.parse::<u32>().ok().filter(|value| *value > 0);
    let height = height.parse::<u32>().ok().filter(|value| *value > 0);
    width.zip(height).ok_or(HlsMasterError::InvalidAttribute {
        attribute: "RESOLUTION",
    })
}

fn parse_positive_f64(value: &str, attribute: &'static str) -> Result<f64, HlsMasterError> {
    value
        .parse::<f64>()
        .ok()
        .filter(|value| value.is_finite() && *value > 0.0)
        .ok_or(HlsMasterError::InvalidAttribute { attribute })
}

#[cfg(test)]
mod tests {
    use super::{
        HlsByteRange, HlsMasterError, HlsMediaError, ProtectedMediaKind, parse_hls_master_playlist,
        parse_hls_media_playlist,
    };
    use crate::domain::HttpUrl;

    fn url(value: &str) -> HttpUrl {
        HttpUrl::parse(value).expect("fixture URL")
    }

    #[test]
    fn variant_audio_and_subtitle_uri_corpus_resolves_with_metadata() {
        let parsed = parse_hls_master_playlist(
            "#EXTM3U\n\
             #EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID=\"aac\",NAME=\"English, stereo\",LANGUAGE=\"en\",DEFAULT=YES,URI=\"audio/en.m3u8\"\n\
             #EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID=\"muxed\",NAME=\"Muxed\"\n\
             #EXT-X-MEDIA:TYPE=SUBTITLES,GROUP-ID=\"subs\",NAME=\"Bangla\",LANGUAGE=\"bn\",URI=\"../text/bn.m3u8?token=1\"\n\
             #EXT-X-STREAM-INF:BANDWIDTH=2800000,RESOLUTION=1920x1080,FRAME-RATE=59.94,CODECS=\"avc1.640028,mp4a.40.2\",AUDIO=\"aac\",SUBTITLES=\"subs\"\n\
             video/main.m3u8\n\
             #EXT-X-STREAM-INF:BANDWIDTH=800000\n\
             https://cdn.example.test/low.m3u8",
            &url("https://media.example.test/live/master/index.m3u8"),
        )
        .expect("master playlist");

        assert_eq!(parsed.variants.len(), 2);
        assert_eq!(
            parsed.variants[0].uri.as_str(),
            "https://media.example.test/live/master/video/main.m3u8"
        );
        assert_eq!(parsed.variants[0].resolution, Some((1920, 1080)));
        assert_eq!(parsed.variants[0].frame_rate, Some(59.94));
        assert_eq!(parsed.variants[0].codecs, ["avc1.640028", "mp4a.40.2"]);
        assert_eq!(
            parsed.variants[1].uri.as_str(),
            "https://cdn.example.test/low.m3u8"
        );
        assert_eq!(parsed.audio_renditions[0].name, "English, stereo");
        assert!(parsed.audio_renditions[0].is_default);
        assert_eq!(parsed.audio_renditions[1].uri, None);
        assert_eq!(
            parsed.subtitle_renditions[0].language.as_deref(),
            Some("bn")
        );
        assert_eq!(
            parsed.subtitle_renditions[0]
                .uri
                .as_ref()
                .map(HttpUrl::as_str),
            Some("https://media.example.test/live/text/bn.m3u8?token=1")
        );
    }

    #[test]
    fn media_playlist_and_malformed_master_inputs_are_rejected() {
        let base = url("https://example.test/master.m3u8");
        let cases = [
            (
                "#EXTM3U\n#EXTINF:4,\nsegment.ts",
                HlsMasterError::NotMasterPlaylist,
            ),
            (
                "#EXTM3U\n#EXT-X-STREAM-INF:RESOLUTION=640x360\nlow.m3u8",
                HlsMasterError::MissingAttribute {
                    tag: "EXT-X-STREAM-INF",
                    attribute: "BANDWIDTH",
                },
            ),
            (
                "#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=0\nlow.m3u8",
                HlsMasterError::InvalidAttribute {
                    attribute: "BANDWIDTH",
                },
            ),
            (
                "#EXTM3U\n#EXT-X-MEDIA:TYPE=SUBTITLES,GROUP-ID=\"s\",NAME=\"English\"",
                HlsMasterError::MissingAttribute {
                    tag: "EXT-X-MEDIA",
                    attribute: "URI",
                },
            ),
            (
                "#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=1,CODECS=\"unterminated\nlow.m3u8",
                HlsMasterError::MalformedTag("EXT-X-STREAM-INF"),
            ),
        ];
        for (manifest, expected) in cases {
            assert_eq!(parse_hls_master_playlist(manifest, &base), Err(expected));
        }
        assert_eq!(
            parse_hls_master_playlist("not hls", &base),
            Err(HlsMasterError::MissingHeader)
        );
    }

    #[test]
    fn non_http_and_missing_stream_uris_are_rejected() {
        let base = url("https://example.test/master.m3u8");
        assert_eq!(
            parse_hls_master_playlist(
                "#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=1\nfile:///secret",
                &base
            ),
            Err(HlsMasterError::InvalidUri)
        );
        assert_eq!(
            parse_hls_master_playlist(
                "#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=1\n#EXT-X-ENDLIST",
                &base
            ),
            Err(HlsMasterError::MalformedTag("EXT-X-STREAM-INF"))
        );
    }

    #[test]
    fn protected_hls_fixtures_are_classified_without_exposing_key_material() {
        let base = url("https://example.test/master.m3u8");
        let master_cases = [
            (
                "#EXTM3U\n#EXT-X-SESSION-KEY:METHOD=SAMPLE-AES,KEYFORMAT=\"com.apple.streamingkeydelivery\",URI=\"skd://license-secret\"\n#EXT-X-STREAM-INF:BANDWIDTH=1\nvideo.m3u8",
                ProtectedMediaKind::FairPlay,
            ),
            (
                "#EXTM3U\n#EXT-X-SESSION-KEY:METHOD=SAMPLE-AES,KEYFORMAT=\"urn:uuid:edef8ba9-79d6-4ace-a3c8-27dcd51d21ed\",URI=\"data:text/plain,pssh-secret\"\n#EXT-X-STREAM-INF:BANDWIDTH=1\nvideo.m3u8",
                ProtectedMediaKind::Widevine,
            ),
        ];
        for (manifest, expected) in master_cases {
            let error = parse_hls_master_playlist(manifest, &base).expect_err("must hard block");
            assert_eq!(error, HlsMasterError::ProtectedMedia(expected));
            let diagnostic = format!("{error:?} {error}");
            assert!(!diagnostic.contains("license-secret"));
            assert!(!diagnostic.contains("pssh-secret"));
        }

        let media_cases = [
            (
                "#EXTM3U\n#EXT-X-TARGETDURATION:4\n#EXT-X-KEY:METHOD=AES-128,URI=\"https://keys.test/key?token=secret\"\n#EXTINF:4,\nsegment.ts\n#EXT-X-ENDLIST",
                ProtectedMediaKind::HlsEncryption,
            ),
            (
                "#EXTM3U\n#EXT-X-TARGETDURATION:4\n#EXT-X-KEY:METHOD=SAMPLE-AES,KEYFORMAT=\"com.microsoft.playready\",URI=\"https://license.test/secret\"\n#EXTINF:4,\nsegment.ts\n#EXT-X-ENDLIST",
                ProtectedMediaKind::PlayReady,
            ),
        ];
        for (manifest, expected) in media_cases {
            assert_eq!(
                parse_hls_media_playlist(manifest, &base),
                Err(HlsMediaError::ProtectedMedia(expected))
            );
        }
    }

    #[test]
    fn explicit_hls_method_none_remains_clear() {
        let parsed = parse_hls_media_playlist(
            "#EXTM3U\n#EXT-X-TARGETDURATION:4\n#EXT-X-KEY:METHOD=NONE\n#EXTINF:4,\nsegment.ts\n#EXT-X-ENDLIST",
            &url("https://example.test/vod/index.m3u8"),
        )
        .expect("unencrypted playlist");
        assert_eq!(parsed.segments.len(), 1);
    }

    #[test]
    fn ts_segments_preserve_sequence_order_and_discontinuities() {
        let parsed = parse_hls_media_playlist(
            "#EXTM3U\n#EXT-X-TARGETDURATION:10\n#EXT-X-MEDIA-SEQUENCE:41\n\
             #EXTINF:9.5,opening\nparts/00041.ts\n#EXT-X-DISCONTINUITY\n\
             #EXTINF:10,\nhttps://cdn.example.test/00042.ts?token=x\n#EXT-X-ENDLIST",
            &url("https://media.example.test/vod/playlist.m3u8"),
        )
        .expect("TS media playlist");

        assert_eq!(parsed.media_sequence, 41);
        assert_eq!(parsed.target_duration, 10);
        assert!(parsed.has_end_list);
        assert_eq!(parsed.segments.len(), 2);
        assert_eq!(parsed.segments[0].sequence, 41);
        assert_eq!(parsed.segments[0].duration_seconds, 9.5);
        assert_eq!(
            parsed.segments[0].uri.as_str(),
            "https://media.example.test/vod/parts/00041.ts"
        );
        assert!(!parsed.segments[0].discontinuity);
        assert_eq!(parsed.segments[1].sequence, 42);
        assert!(parsed.segments[1].discontinuity);
    }

    #[test]
    fn fmp4_init_and_implicit_byte_ranges_follow_declared_state() {
        let parsed = parse_hls_media_playlist(
            "#EXTM3U\n#EXT-X-TARGETDURATION:4\n\
             #EXT-X-MAP:URI=\"media.mp4\",BYTERANGE=\"20@0\"\n\
             #EXTINF:4,\n#EXT-X-BYTERANGE:100@20\nmedia.mp4\n\
             #EXTINF:4,\n#EXT-X-BYTERANGE:80\nmedia.mp4\n\
             #EXT-X-MAP:URI=\"second.mp4\",BYTERANGE=\"16@4\"\n\
             #EXTINF:2,\nsecond.mp4\n#EXT-X-ENDLIST",
            &url("https://example.test/video/index.m3u8"),
        )
        .expect("fMP4 media playlist");

        assert_eq!(
            parsed.segments[0].byte_range,
            Some(HlsByteRange {
                offset: 20,
                length: 100
            })
        );
        assert_eq!(
            parsed.segments[1].byte_range,
            Some(HlsByteRange {
                offset: 120,
                length: 80
            })
        );
        assert_eq!(
            parsed.segments[0].init_segment,
            parsed.segments[1].init_segment
        );
        let first_init = parsed.segments[0]
            .init_segment
            .as_ref()
            .expect("first init");
        assert_eq!(
            first_init.uri.as_str(),
            "https://example.test/video/media.mp4"
        );
        assert_eq!(
            first_init.byte_range,
            Some(HlsByteRange {
                offset: 0,
                length: 20
            })
        );
        let replacement = parsed.segments[2]
            .init_segment
            .as_ref()
            .expect("replacement init");
        assert_eq!(
            replacement.uri.as_str(),
            "https://example.test/video/second.mp4"
        );
    }

    #[test]
    fn malformed_media_playlist_corpus_is_rejected() {
        let base = url("https://example.test/vod/index.m3u8");
        let cases = [
            ("not hls", HlsMediaError::MissingHeader),
            (
                "#EXTM3U\n#EXT-X-TARGETDURATION:4\n#EXT-X-STREAM-INF:BANDWIDTH=1\nmaster.m3u8",
                HlsMediaError::MasterPlaylist,
            ),
            (
                "#EXTM3U\n#EXTINF:4,\nsegment.ts\n#EXT-X-ENDLIST",
                HlsMediaError::MissingAttribute {
                    tag: "playlist",
                    attribute: "EXT-X-TARGETDURATION",
                },
            ),
            (
                "#EXTM3U\n#EXT-X-TARGETDURATION:4\n#EXTINF:0,\nsegment.ts",
                HlsMediaError::InvalidAttribute {
                    attribute: "EXTINF duration",
                },
            ),
            (
                "#EXTM3U\n#EXT-X-TARGETDURATION:4\n#EXTINF:4,\n#EXT-X-BYTERANGE:10\nblob.ts",
                HlsMediaError::MissingByteRangeOffset,
            ),
            (
                "#EXTM3U\n#EXT-X-TARGETDURATION:4\n#EXTINF:4,\nfile:///secret",
                HlsMediaError::InvalidUri,
            ),
            (
                "#EXTM3U\n#EXT-X-TARGETDURATION:4\n#EXTINF:4,",
                HlsMediaError::MalformedTag("segment"),
            ),
        ];
        for (manifest, expected) in cases {
            assert_eq!(parse_hls_media_playlist(manifest, &base), Err(expected));
        }
    }
}
