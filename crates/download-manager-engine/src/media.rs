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
}

/// Parses an HLS master playlist and resolves every media URI against its URL.
///
/// Media-playlist segments are intentionally outside this contract.
///
/// # Errors
/// Returns an error for a non-master playlist or malformed required metadata/URI.
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
            let entry = input[start..index].trim();
            let (name, raw) = entry
                .split_once('=')
                .ok_or(HlsMasterError::MalformedTag(tag))?;
            let value = raw
                .strip_prefix('"')
                .and_then(|value| value.strip_suffix('"'))
                .unwrap_or(raw);
            if name.is_empty()
                || value.is_empty()
                || values.insert(name.to_owned(), value.to_owned()).is_some()
            {
                return Err(HlsMasterError::MalformedTag(tag));
            }
            start = index + 1;
        }
    }
    if quoted || values.is_empty() {
        return Err(HlsMasterError::MalformedTag(tag));
    }
    Ok(values)
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
    use super::{HlsMasterError, parse_hls_master_playlist};
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
}
