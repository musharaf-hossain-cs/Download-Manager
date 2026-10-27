//! Unified, manifest-backed media variant menu and exact track selection.

use crate::{
    dash::{DashAdaptation, DashManifest, DashRepresentation, DashSegmentPlan},
    domain::HttpUrl,
    media::{HlsMasterPlaylist, HlsRendition},
};
use thiserror::Error;

/// Maximum combined number of menu rows accepted from one manifest.
pub const MAX_MEDIA_VARIANT_OPTIONS: usize = 4_096;

/// The role a selectable manifest track has in the final output.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaTrackKind {
    Video,
    Audio,
    Subtitle,
}

/// An exact, immutable reference back to one parsed manifest track.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MediaTrackRef {
    HlsVariant {
        index: usize,
        uri: HttpUrl,
        audio_group: Option<String>,
        subtitle_group: Option<String>,
    },
    HlsRendition {
        kind: MediaTrackKind,
        index: usize,
        group_id: String,
        name: String,
        uri: Option<HttpUrl>,
    },
    DashRepresentation {
        period_index: usize,
        adaptation_index: usize,
        representation_index: usize,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaSizeAccuracy {
    Known,
    Estimated,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MediaSize {
    pub bytes: u64,
    pub accuracy: MediaSizeAccuracy,
}

/// One row in a quality, language, or subtitle menu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MediaVariantOption {
    pub id: String,
    pub kind: MediaTrackKind,
    pub track: MediaTrackRef,
    pub resolution: Option<(u32, u32)>,
    pub frame_rate_millihertz: Option<u64>,
    pub bitrate: Option<u64>,
    pub codecs: Vec<String>,
    pub container: Option<String>,
    pub language: Option<String>,
    pub name: Option<String>,
    pub size: Option<MediaSize>,
    pub is_default: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct MediaVariantMenu {
    pub video: Vec<MediaVariantOption>,
    pub audio: Vec<MediaVariantOption>,
    pub subtitles: Vec<MediaVariantOption>,
}

/// Exact manifest tracks selected from a [`MediaVariantMenu`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectedMediaTracks {
    pub video: Option<MediaTrackRef>,
    pub audio: Option<MediaTrackRef>,
    pub subtitle: Option<MediaTrackRef>,
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum MediaSelectionError {
    #[error("media manifest exceeds the 4096-option menu limit")]
    TooManyOptions,
    #[error("at least one video or audio track must be selected")]
    EmptySelection,
    #[error("unknown {kind:?} option `{id}`")]
    UnknownOption { kind: MediaTrackKind, id: String },
    #[error("selected tracks do not belong to the same manifest variant scope")]
    IncompatibleTracks,
}

impl MediaVariantMenu {
    /// Builds an HLS menu in manifest order. A duration enables bandwidth-based size estimates.
    ///
    /// # Errors
    /// Returns an error when the combined menu would exceed [`MAX_MEDIA_VARIANT_OPTIONS`].
    pub fn from_hls(
        master: &HlsMasterPlaylist,
        duration_millis: Option<u64>,
    ) -> Result<Self, MediaSelectionError> {
        let option_count = master
            .variants
            .len()
            .checked_add(master.audio_renditions.len())
            .and_then(|count| count.checked_add(master.subtitle_renditions.len()))
            .ok_or(MediaSelectionError::TooManyOptions)?;
        if option_count > MAX_MEDIA_VARIANT_OPTIONS {
            return Err(MediaSelectionError::TooManyOptions);
        }
        let mut video = Vec::new();
        let mut audio = hls_rendition_options(&master.audio_renditions, MediaTrackKind::Audio, "a");
        for (index, variant) in master.variants.iter().enumerate() {
            let kind = hls_variant_kind(variant.resolution, &variant.codecs);
            let option = MediaVariantOption {
                id: format!(
                    "hls:{}:{index}",
                    if kind == MediaTrackKind::Audio {
                        "av"
                    } else {
                        "v"
                    }
                ),
                kind,
                track: MediaTrackRef::HlsVariant {
                    index,
                    uri: variant.uri.clone(),
                    audio_group: variant.audio_group.clone(),
                    subtitle_group: variant.subtitle_group.clone(),
                },
                resolution: variant.resolution,
                frame_rate_millihertz: variant.frame_rate.and_then(rate_to_millihertz),
                bitrate: Some(variant.bandwidth),
                codecs: variant.codecs.clone(),
                container: None,
                language: None,
                name: None,
                size: duration_millis
                    .and_then(|duration| estimate_size(variant.bandwidth, duration)),
                is_default: false,
            };
            match kind {
                MediaTrackKind::Video => video.push(option),
                MediaTrackKind::Audio => audio.push(option),
                MediaTrackKind::Subtitle => unreachable!("HLS variants are audio or video"),
            }
        }
        let subtitles =
            hls_rendition_options(&master.subtitle_renditions, MediaTrackKind::Subtitle, "s");
        Ok(Self {
            video,
            audio,
            subtitles,
        })
    }

    /// Builds a DASH menu in period/adaptation/representation order.
    ///
    /// # Errors
    /// Returns an error when the combined menu would exceed [`MAX_MEDIA_VARIANT_OPTIONS`].
    pub fn from_dash(manifest: &DashManifest) -> Result<Self, MediaSelectionError> {
        let mut menu = Self::default();
        let mut option_count = 0_usize;
        for (period_index, period) in manifest.periods.iter().enumerate() {
            for (adaptation_index, adaptation) in period.adaptations.iter().enumerate() {
                for (representation_index, representation) in
                    adaptation.representations.iter().enumerate()
                {
                    let Some(kind) = dash_kind(adaptation, representation) else {
                        continue;
                    };
                    option_count = option_count
                        .checked_add(1)
                        .ok_or(MediaSelectionError::TooManyOptions)?;
                    if option_count > MAX_MEDIA_VARIANT_OPTIONS {
                        return Err(MediaSelectionError::TooManyOptions);
                    }
                    let option = dash_option(
                        kind,
                        period_index,
                        adaptation_index,
                        representation_index,
                        period.duration_millis,
                        adaptation,
                        representation,
                    );
                    match kind {
                        MediaTrackKind::Video => menu.video.push(option),
                        MediaTrackKind::Audio => menu.audio.push(option),
                        MediaTrackKind::Subtitle => menu.subtitles.push(option),
                    }
                }
            }
        }
        Ok(menu)
    }

    /// Resolves menu IDs to their exact source tracks and rejects cross-variant combinations.
    ///
    /// # Errors
    /// Returns an error for an empty selection, unknown/wrong-kind IDs, HLS group mismatches, or
    /// DASH tracks taken from different periods.
    pub fn select(
        &self,
        video_id: Option<&str>,
        audio_id: Option<&str>,
        subtitle_id: Option<&str>,
    ) -> Result<SelectedMediaTracks, MediaSelectionError> {
        if video_id.is_none() && audio_id.is_none() {
            return Err(MediaSelectionError::EmptySelection);
        }
        let video = resolve(&self.video, video_id, MediaTrackKind::Video)?;
        let audio = resolve(&self.audio, audio_id, MediaTrackKind::Audio)?;
        let subtitle = resolve(&self.subtitles, subtitle_id, MediaTrackKind::Subtitle)?;
        if !compatible(video, audio, subtitle) {
            return Err(MediaSelectionError::IncompatibleTracks);
        }
        Ok(SelectedMediaTracks {
            video: video.map(|option| option.track.clone()),
            audio: audio.map(|option| option.track.clone()),
            subtitle: subtitle.map(|option| option.track.clone()),
        })
    }
}

fn hls_rendition_options(
    renditions: &[HlsRendition],
    kind: MediaTrackKind,
    id_kind: &str,
) -> Vec<MediaVariantOption> {
    renditions
        .iter()
        .enumerate()
        .map(|(index, rendition)| MediaVariantOption {
            id: format!("hls:{id_kind}:{index}"),
            kind,
            track: MediaTrackRef::HlsRendition {
                kind,
                index,
                group_id: rendition.group_id.clone(),
                name: rendition.name.clone(),
                uri: rendition.uri.clone(),
            },
            resolution: None,
            frame_rate_millihertz: None,
            bitrate: None,
            codecs: Vec::new(),
            container: None,
            language: rendition.language.clone(),
            name: Some(rendition.name.clone()),
            size: None,
            is_default: rendition.is_default,
        })
        .collect()
}

fn hls_variant_kind(resolution: Option<(u32, u32)>, codecs: &[String]) -> MediaTrackKind {
    let audio_only = resolution.is_none()
        && !codecs.is_empty()
        && codecs.iter().all(|codec| {
            matches!(
                codec.trim().to_ascii_lowercase().split('.').next(),
                Some("aac" | "ac-3" | "ec-3" | "flac" | "mp3" | "mp4a" | "opus" | "vorbis")
            )
        });
    if audio_only {
        MediaTrackKind::Audio
    } else {
        MediaTrackKind::Video
    }
}

fn dash_option(
    kind: MediaTrackKind,
    period_index: usize,
    adaptation_index: usize,
    representation_index: usize,
    duration_millis: Option<u64>,
    adaptation: &DashAdaptation,
    representation: &DashRepresentation,
) -> MediaVariantOption {
    let mime = representation
        .mime_type
        .as_deref()
        .or(adaptation.mime_type.as_deref());
    let codecs = representation
        .codecs
        .as_deref()
        .or(adaptation.codecs.as_deref())
        .map(split_codecs)
        .unwrap_or_default();
    let size = known_dash_size(representation.segment_plan.as_ref()).or_else(|| {
        representation
            .bandwidth
            .zip(duration_millis)
            .and_then(|(bitrate, duration)| estimate_size(bitrate, duration))
    });
    MediaVariantOption {
        id: format!("dash:p{period_index}:a{adaptation_index}:r{representation_index}"),
        kind,
        track: MediaTrackRef::DashRepresentation {
            period_index,
            adaptation_index,
            representation_index,
        },
        resolution: representation.width.zip(representation.height),
        frame_rate_millihertz: representation
            .frame_rate
            .as_deref()
            .and_then(parse_dash_frame_rate),
        bitrate: representation.bandwidth,
        codecs,
        container: container_from_mime(mime).map(str::to_owned),
        language: adaptation.language.clone(),
        name: representation.id.clone(),
        size,
        is_default: false,
    }
}

fn dash_kind(
    adaptation: &DashAdaptation,
    representation: &DashRepresentation,
) -> Option<MediaTrackKind> {
    let mime = representation
        .mime_type
        .as_deref()
        .or(adaptation.mime_type.as_deref());
    let declared = adaptation
        .content_type
        .as_deref()
        .or_else(|| mime.and_then(|value| value.split_once('/').map(|(prefix, _)| prefix)));
    match declared {
        Some("video") => Some(MediaTrackKind::Video),
        Some("audio") => Some(MediaTrackKind::Audio),
        Some("text" | "subtitle" | "subtitles") => Some(MediaTrackKind::Subtitle),
        _ if mime.is_some_and(|value| value.split(';').next() == Some("application/ttml+xml")) => {
            Some(MediaTrackKind::Subtitle)
        }
        _ if representation
            .codecs
            .as_deref()
            .or(adaptation.codecs.as_deref())
            .is_some_and(|codecs| {
                codecs
                    .split(',')
                    .any(|codec| matches!(codec.trim(), "wvtt" | "stpp"))
            }) =>
        {
            Some(MediaTrackKind::Subtitle)
        }
        _ => None,
    }
}

fn resolve<'a>(
    options: &'a [MediaVariantOption],
    id: Option<&str>,
    kind: MediaTrackKind,
) -> Result<Option<&'a MediaVariantOption>, MediaSelectionError> {
    id.map(|id| {
        options
            .iter()
            .find(|option| option.id == id)
            .ok_or_else(|| MediaSelectionError::UnknownOption {
                kind,
                id: id.to_owned(),
            })
    })
    .transpose()
}

fn compatible(
    video: Option<&MediaVariantOption>,
    audio: Option<&MediaVariantOption>,
    subtitle: Option<&MediaVariantOption>,
) -> bool {
    let tracks = [video, audio, subtitle];
    let dash_period = tracks
        .iter()
        .flatten()
        .find_map(|option| match option.track {
            MediaTrackRef::DashRepresentation { period_index, .. } => Some(period_index),
            _ => None,
        });
    if let Some(period) = dash_period {
        return tracks.iter().flatten().all(|option| {
            matches!(option.track, MediaTrackRef::DashRepresentation { period_index, .. } if period_index == period)
        });
    }
    let hls_variant = video
        .or_else(|| audio.filter(|option| matches!(option.track, MediaTrackRef::HlsVariant { .. })))
        .map(|option| &option.track);
    let Some(MediaTrackRef::HlsVariant {
        audio_group,
        subtitle_group,
        ..
    }) = hls_variant
    else {
        return subtitle.is_none()
            && audio.is_some_and(|option| {
                matches!(
                    option.track,
                    MediaTrackRef::HlsRendition {
                        kind: MediaTrackKind::Audio,
                        uri: Some(_),
                        ..
                    }
                )
            });
    };
    let audio_matches = video.is_none() || hls_group_matches(audio, audio_group.as_deref());
    audio_matches && hls_group_matches(subtitle, subtitle_group.as_deref())
}

fn hls_group_matches(option: Option<&MediaVariantOption>, expected: Option<&str>) -> bool {
    option.is_none_or(|option| {
        matches!(&option.track, MediaTrackRef::HlsRendition { group_id, .. } if Some(group_id.as_str()) == expected)
    })
}

fn estimate_size(bitrate: u64, duration_millis: u64) -> Option<MediaSize> {
    let bits_millis = u128::from(bitrate).checked_mul(u128::from(duration_millis))?;
    let bytes = u64::try_from(bits_millis.div_ceil(8_000)).ok()?;
    Some(MediaSize {
        bytes,
        accuracy: MediaSizeAccuracy::Estimated,
    })
}

fn known_dash_size(plan: Option<&DashSegmentPlan>) -> Option<MediaSize> {
    let plan = plan?;
    let mut bytes = 0_u64;
    if let Some(initialization) = &plan.initialization {
        bytes = bytes.checked_add(dash_range_length(initialization.byte_range?)?)?;
    }
    for segment in &plan.media {
        bytes = bytes.checked_add(dash_range_length(segment.resource.byte_range?)?)?;
    }
    Some(MediaSize {
        bytes,
        accuracy: MediaSizeAccuracy::Known,
    })
}

fn rate_to_millihertz(rate: f64) -> Option<u64> {
    if rate.is_finite() && (0.0..=1_000_000.0).contains(&rate) {
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        Some((rate * 1_000.0).round() as u64)
    } else {
        None
    }
}

fn parse_dash_frame_rate(value: &str) -> Option<u64> {
    let (numerator, denominator) = value.split_once('/').unwrap_or((value, "1"));
    let numerator = numerator.parse::<u64>().ok()?;
    let denominator = denominator.parse::<u64>().ok()?;
    numerator
        .checked_mul(1_000)?
        .checked_add(denominator / 2)?
        .checked_div(denominator)
}

fn split_codecs(codecs: &str) -> Vec<String> {
    codecs
        .split(',')
        .map(str::trim)
        .filter(|codec| !codec.is_empty())
        .map(str::to_owned)
        .collect()
}

fn container_from_mime(mime: Option<&str>) -> Option<&'static str> {
    match mime?.split(';').next()?.trim() {
        "video/mp4" | "audio/mp4" | "application/mp4" => Some("mp4"),
        "video/webm" | "audio/webm" => Some("webm"),
        "video/mp2t" => Some("mpeg-ts"),
        "text/vtt" => Some("webvtt"),
        "application/ttml+xml" => Some("ttml"),
        _ => None,
    }
}

fn dash_range_length(range: crate::dash::DashByteRange) -> Option<u64> {
    range.end.checked_sub(range.start)?.checked_add(1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        dash::{DashByteRange, DashMediaSegment, DashPeriod, DashSegmentResource},
        media::{HlsRendition, HlsVariant},
    };
    use url::Url;

    fn http_url(value: &str) -> HttpUrl {
        HttpUrl::parse(value.to_owned()).expect("valid fixture URL")
    }

    fn dash_resource(url: &str, range: Option<(u64, u64)>) -> DashSegmentResource {
        DashSegmentResource {
            url: Url::parse(url).expect("valid fixture URL"),
            byte_range: range.map(|(start, end)| DashByteRange { start, end }),
        }
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn hls_menu_preserves_quality_language_codec_size_and_exact_tracks() {
        let master = HlsMasterPlaylist {
            variants: vec![
                HlsVariant {
                    uri: http_url("https://media.test/1080.m3u8"),
                    bandwidth: 4_000_000,
                    resolution: Some((1920, 1080)),
                    frame_rate: Some(29.97),
                    codecs: vec!["avc1.640028".to_owned(), "mp4a.40.2".to_owned()],
                    audio_group: Some("audio-main".to_owned()),
                    subtitle_group: Some("subs".to_owned()),
                },
                HlsVariant {
                    uri: http_url("https://media.test/720.m3u8"),
                    bandwidth: 2_000_000,
                    resolution: Some((1280, 720)),
                    frame_rate: Some(25.0),
                    codecs: vec!["avc1.4d401f".to_owned()],
                    audio_group: Some("audio-alt".to_owned()),
                    subtitle_group: None,
                },
                HlsVariant {
                    uri: http_url("https://media.test/audio-only.m3u8"),
                    bandwidth: 96_000,
                    resolution: None,
                    frame_rate: None,
                    codecs: vec!["opus".to_owned()],
                    audio_group: None,
                    subtitle_group: None,
                },
            ],
            audio_renditions: vec![
                HlsRendition {
                    group_id: "audio-main".to_owned(),
                    name: "Bangla".to_owned(),
                    language: Some("bn".to_owned()),
                    uri: Some(http_url("https://media.test/audio-bn.m3u8")),
                    is_default: true,
                },
                HlsRendition {
                    group_id: "audio-alt".to_owned(),
                    name: "English".to_owned(),
                    language: Some("en".to_owned()),
                    uri: Some(http_url("https://media.test/audio-en.m3u8")),
                    is_default: false,
                },
                HlsRendition {
                    group_id: "audio-main".to_owned(),
                    name: "Muxed".to_owned(),
                    language: Some("en".to_owned()),
                    uri: None,
                    is_default: false,
                },
            ],
            subtitle_renditions: vec![HlsRendition {
                group_id: "subs".to_owned(),
                name: "বাংলা".to_owned(),
                language: Some("bn".to_owned()),
                uri: Some(http_url("https://media.test/subs-bn.m3u8")),
                is_default: false,
            }],
        };

        let menu = MediaVariantMenu::from_hls(&master, Some(120_000)).expect("bounded menu");
        assert_eq!(menu.video[0].id, "hls:v:0");
        assert_eq!(menu.video[0].resolution, Some((1920, 1080)));
        assert_eq!(menu.video[0].frame_rate_millihertz, Some(29_970));
        assert_eq!(menu.video[0].codecs, ["avc1.640028", "mp4a.40.2"]);
        assert_eq!(
            menu.video[0].size,
            Some(MediaSize {
                bytes: 60_000_000,
                accuracy: MediaSizeAccuracy::Estimated,
            })
        );
        assert_eq!(menu.audio[0].language.as_deref(), Some("bn"));
        assert!(menu.audio[0].is_default);
        assert_eq!(menu.audio[3].id, "hls:av:2");
        assert_eq!(menu.audio[3].codecs, ["opus"]);

        let selected = menu
            .select(Some("hls:v:0"), Some("hls:a:0"), Some("hls:s:0"))
            .expect("compatible tracks");
        assert_eq!(
            selected.audio,
            Some(MediaTrackRef::HlsRendition {
                kind: MediaTrackKind::Audio,
                index: 0,
                group_id: "audio-main".to_owned(),
                name: "Bangla".to_owned(),
                uri: Some(http_url("https://media.test/audio-bn.m3u8")),
            })
        );
        assert_eq!(
            menu.select(Some("hls:v:0"), Some("hls:a:1"), None),
            Err(MediaSelectionError::IncompatibleTracks)
        );
        assert!(matches!(
            menu.select(None, Some("hls:av:2"), None),
            Ok(SelectedMediaTracks {
                video: None,
                audio: Some(MediaTrackRef::HlsVariant { index: 2, .. }),
                subtitle: None,
            })
        ));
        assert_eq!(
            menu.select(None, Some("hls:a:2"), None),
            Err(MediaSelectionError::IncompatibleTracks)
        );
        assert!(menu.select(Some("hls:v:0"), Some("hls:a:2"), None).is_ok());
    }

    #[test]
    fn dash_menu_uses_known_ranges_then_estimates_and_preserves_period_scope() {
        let manifest = DashManifest {
            base_url: Url::parse("https://media.test/manifest.mpd").unwrap(),
            periods: vec![
                DashPeriod {
                    id: Some("main".to_owned()),
                    base_url: Url::parse("https://media.test/").unwrap(),
                    duration_millis: Some(10_000),
                    adaptations: vec![
                        adaptation(
                            Some("video"),
                            "video/mp4",
                            None,
                            representation(
                                "1080",
                                4_000_000,
                                Some((1920, 1080)),
                                Some("30000/1001"),
                                "avc1.640028",
                                Some(DashSegmentPlan {
                                    initialization: Some(dash_resource(
                                        "https://media.test/init.mp4",
                                        Some((0, 99)),
                                    )),
                                    media: vec![DashMediaSegment {
                                        number: 1,
                                        time: 0,
                                        duration: 10,
                                        resource: dash_resource(
                                            "https://media.test/video.m4s",
                                            Some((100, 1099)),
                                        ),
                                    }],
                                }),
                            ),
                        ),
                        adaptation(
                            Some("audio"),
                            "audio/mp4",
                            Some("bn"),
                            representation("bn-128", 128_000, None, None, "mp4a.40.2", None),
                        ),
                        adaptation(
                            None,
                            "application/ttml+xml",
                            Some("en"),
                            representation("captions", 1_000, None, None, "wvtt", None),
                        ),
                    ],
                },
                DashPeriod {
                    id: Some("credits".to_owned()),
                    base_url: Url::parse("https://media.test/").unwrap(),
                    duration_millis: Some(1_000),
                    adaptations: vec![adaptation(
                        Some("audio"),
                        "audio/webm",
                        Some("en"),
                        representation("credits", 64_000, None, None, "opus", None),
                    )],
                },
            ],
        };

        let menu = MediaVariantMenu::from_dash(&manifest).expect("bounded menu");
        assert_eq!(menu.video[0].frame_rate_millihertz, Some(29_970));
        assert_eq!(menu.video[0].container.as_deref(), Some("mp4"));
        assert_eq!(
            menu.video[0].size,
            Some(MediaSize {
                bytes: 1_100,
                accuracy: MediaSizeAccuracy::Known,
            })
        );
        assert_eq!(menu.audio[0].language.as_deref(), Some("bn"));
        assert_eq!(menu.subtitles[0].container.as_deref(), Some("ttml"));
        assert_eq!(
            menu.audio[0].size,
            Some(MediaSize {
                bytes: 160_000,
                accuracy: MediaSizeAccuracy::Estimated,
            })
        );
        let selected = menu
            .select(
                Some("dash:p0:a0:r0"),
                Some("dash:p0:a1:r0"),
                Some("dash:p0:a2:r0"),
            )
            .expect("same-period tracks");
        assert_eq!(
            selected.video,
            Some(MediaTrackRef::DashRepresentation {
                period_index: 0,
                adaptation_index: 0,
                representation_index: 0,
            })
        );
        assert_eq!(
            menu.select(Some("dash:p0:a0:r0"), Some("dash:p1:a0:r0"), None),
            Err(MediaSelectionError::IncompatibleTracks)
        );
    }

    #[test]
    fn selection_rejects_empty_unknown_and_wrong_kind_ids() {
        let menu = MediaVariantMenu {
            video: vec![MediaVariantOption {
                id: "v".to_owned(),
                kind: MediaTrackKind::Video,
                track: MediaTrackRef::HlsVariant {
                    index: 0,
                    uri: http_url("https://media.test/video.m3u8"),
                    audio_group: None,
                    subtitle_group: None,
                },
                resolution: None,
                frame_rate_millihertz: None,
                bitrate: None,
                codecs: Vec::new(),
                container: None,
                language: None,
                name: None,
                size: None,
                is_default: false,
            }],
            ..MediaVariantMenu::default()
        };
        assert_eq!(
            menu.select(None, None, Some("missing")),
            Err(MediaSelectionError::EmptySelection)
        );
        assert_eq!(
            menu.select(Some("missing"), None, None),
            Err(MediaSelectionError::UnknownOption {
                kind: MediaTrackKind::Video,
                id: "missing".to_owned(),
            })
        );
        assert_eq!(
            menu.select(None, Some("v"), None),
            Err(MediaSelectionError::UnknownOption {
                kind: MediaTrackKind::Audio,
                id: "v".to_owned(),
            })
        );
    }

    #[test]
    fn menu_limits_and_size_overflow_are_explicit() {
        let variant = HlsVariant {
            uri: http_url("https://media.test/video.m3u8"),
            bandwidth: 1,
            resolution: None,
            frame_rate: None,
            codecs: Vec::new(),
            audio_group: None,
            subtitle_group: None,
        };
        let oversized = HlsMasterPlaylist {
            variants: vec![variant; MAX_MEDIA_VARIANT_OPTIONS + 1],
            audio_renditions: Vec::new(),
            subtitle_renditions: Vec::new(),
        };
        assert_eq!(
            MediaVariantMenu::from_hls(&oversized, None),
            Err(MediaSelectionError::TooManyOptions)
        );

        let overflow_plan = DashSegmentPlan {
            initialization: Some(dash_resource(
                "https://media.test/full.bin",
                Some((0, u64::MAX)),
            )),
            media: Vec::new(),
        };
        assert_eq!(known_dash_size(Some(&overflow_plan)), None);
        assert_eq!(estimate_size(u64::MAX, u64::MAX), None);
        assert_eq!(
            estimate_size(u64::MAX, 1),
            Some(MediaSize {
                bytes: u64::try_from(u128::from(u64::MAX).div_ceil(8_000)).unwrap(),
                accuracy: MediaSizeAccuracy::Estimated,
            })
        );
    }

    fn adaptation(
        content_type: Option<&str>,
        mime_type: &str,
        language: Option<&str>,
        representation: DashRepresentation,
    ) -> DashAdaptation {
        DashAdaptation {
            id: None,
            content_type: content_type.map(str::to_owned),
            mime_type: Some(mime_type.to_owned()),
            language: language.map(str::to_owned),
            codecs: None,
            base_url: Url::parse("https://media.test/").unwrap(),
            content_protections: Vec::new(),
            representations: vec![representation],
        }
    }

    fn representation(
        id: &str,
        bandwidth: u64,
        dimensions: Option<(u32, u32)>,
        frame_rate: Option<&str>,
        codecs: &str,
        segment_plan: Option<DashSegmentPlan>,
    ) -> DashRepresentation {
        DashRepresentation {
            id: Some(id.to_owned()),
            bandwidth: Some(bandwidth),
            width: dimensions.map(|value| value.0),
            height: dimensions.map(|value| value.1),
            frame_rate: frame_rate.map(str::to_owned),
            mime_type: None,
            codecs: Some(codecs.to_owned()),
            base_url: Url::parse("https://media.test/").unwrap(),
            content_protections: Vec::new(),
            segment_plan,
        }
    }
}
