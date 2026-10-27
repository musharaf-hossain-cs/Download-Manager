//! Source-neutral audio-only download and output planning.

use std::path::Path;

use thiserror::Error;

use crate::{
    dash_download::{DashTrackDownloadError, DashTrackDownloader, DashTrackRequest},
    domain::{HttpUrl, UnixMillis},
    hls::{HlsDownloadError, HlsVodDownloader},
    media::HlsMediaPlaylist,
    media_mux::AudioCodec,
    media_worker::FfmpegContainer,
    transfer::{SingleStreamDownloader, TransferError},
};

/// The media source used to produce an audio-only output.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioSourceKind {
    Direct,
    Hls,
    Dash,
}

/// A deterministic copy-only container decision for an audio track.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AudioOutputPlan {
    pub source: AudioSourceKind,
    pub codec: AudioCodec,
    pub container: Option<FfmpegContainer>,
    pub extension: &'static str,
}

impl AudioOutputPlan {
    /// Preserves a direct audio object's validated extension and exact bytes.
    ///
    /// # Errors
    /// Returns an error when the extension is not one of the progressive audio types accepted by
    /// the browser handoff contract.
    pub fn direct(codec: AudioCodec, extension: &str) -> Result<Self, AudioOutputPlanError> {
        let extension = extension
            .trim()
            .trim_start_matches('.')
            .to_ascii_lowercase();
        let extension = match extension.as_str() {
            "aac" => "aac",
            "flac" => "flac",
            "m4a" => "m4a",
            "mp3" => "mp3",
            "oga" => "oga",
            "ogg" => "ogg",
            "opus" => "opus",
            "weba" => "weba",
            _ => return Err(AudioOutputPlanError::UnsupportedDirectExtension(extension)),
        };
        Ok(Self {
            source: AudioSourceKind::Direct,
            codec,
            container: None,
            extension,
        })
    }

    /// Chooses a lossless, copy-only output container for an HLS or DASH audio track.
    ///
    /// AAC, AC-3, E-AC-3, and MP3 use the MP4/M4A family; Opus and Vorbis use `WebM`; FLAC uses
    /// Matroska. The original encoded frames are never transcoded.
    ///
    /// # Errors
    /// Returns an error when called for a direct source, which must preserve its original format.
    pub const fn segmented(
        source: AudioSourceKind,
        codec: AudioCodec,
    ) -> Result<Self, AudioOutputPlanError> {
        if matches!(source, AudioSourceKind::Direct) {
            return Err(AudioOutputPlanError::DirectRequiresExtension);
        }
        let (container, extension) = match codec {
            AudioCodec::Aac | AudioCodec::Ac3 | AudioCodec::Eac3 | AudioCodec::Mp3 => {
                (FfmpegContainer::Mp4, "m4a")
            }
            AudioCodec::Opus | AudioCodec::Vorbis => (FfmpegContainer::WebM, "weba"),
            AudioCodec::Flac => (FfmpegContainer::Matroska, "mka"),
        };
        Ok(Self {
            source,
            codec,
            container: Some(container),
            extension,
        })
    }
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum AudioOutputPlanError {
    #[error("unsupported direct audio extension: {0}")]
    UnsupportedDirectExtension(String),
    #[error("direct audio output must preserve a validated source extension")]
    DirectRequiresExtension,
}

/// Exact byte/hash evidence returned by every audio-only source path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioOutputOutcome {
    pub source: AudioSourceKind,
    pub downloaded_bytes: u64,
    pub skipped_units: usize,
    pub downloaded_units: usize,
    pub sha256: String,
}

#[derive(Debug, Error)]
pub enum AudioOutputError {
    #[error("direct audio download failed: {0}")]
    Direct(#[from] TransferError),
    #[error("HLS audio download failed: {0}")]
    Hls(#[from] HlsDownloadError),
    #[error("DASH audio download failed: {0}")]
    Dash(#[from] DashTrackDownloadError),
}

/// Routes direct, HLS, and DASH audio-only selections through their established downloaders.
#[derive(Debug, Clone)]
pub struct AudioOnlyDownloader {
    direct: SingleStreamDownloader,
    hls: HlsVodDownloader,
    dash: DashTrackDownloader,
}

impl AudioOnlyDownloader {
    /// Creates all three bounded HTTP clients.
    ///
    /// # Errors
    /// Returns an error if any underlying client cannot be initialized.
    pub fn new() -> Result<Self, AudioOutputError> {
        Ok(Self {
            direct: SingleStreamDownloader::new()?,
            hls: HlsVodDownloader::new()?,
            dash: DashTrackDownloader::new()?,
        })
    }

    /// Downloads a progressive audio object unchanged.
    ///
    /// # Errors
    /// Returns an HTTP, length, disk, or filesystem error from the direct transfer.
    pub fn download_direct(
        &self,
        url: &HttpUrl,
        output: &Path,
    ) -> Result<AudioOutputOutcome, AudioOutputError> {
        let outcome = self.direct.download(url, output, |_| {})?;
        Ok(AudioOutputOutcome {
            source: AudioSourceKind::Direct,
            downloaded_bytes: outcome.downloaded_bytes,
            skipped_units: 0,
            downloaded_units: 1,
            sha256: outcome.sha256,
        })
    }

    /// Downloads a selected audio-only HLS VOD playlist in manifest order.
    ///
    /// # Errors
    /// Returns a non-VOD, HTTP, range, retry, disk, or filesystem error from the HLS transfer.
    pub fn download_hls(
        &self,
        playlist: &HlsMediaPlaylist,
        output: &Path,
    ) -> Result<AudioOutputOutcome, AudioOutputError> {
        let outcome = self.hls.download(playlist, output)?;
        Ok(AudioOutputOutcome {
            source: AudioSourceKind::Hls,
            downloaded_bytes: outcome.downloaded_bytes,
            skipped_units: 0,
            downloaded_units: outcome.init_segments + outcome.media_segments,
            sha256: outcome.sha256,
        })
    }

    /// Downloads/resumes only the selected DASH audio representation.
    ///
    /// # Errors
    /// Returns a selection, HTTP, range, retry, checkpoint, storage, or filesystem error from the
    /// DASH transfer.
    pub fn resume_dash(
        &self,
        database_path: &Path,
        audio: DashTrackRequest,
        at: UnixMillis,
    ) -> Result<AudioOutputOutcome, AudioOutputError> {
        let outcome = self.dash.resume_audio(database_path, audio, at)?;
        Ok(AudioOutputOutcome {
            source: AudioSourceKind::Dash,
            downloaded_bytes: outcome.downloaded_bytes,
            skipped_units: outcome.skipped_units,
            downloaded_units: outcome.downloaded_units,
            sha256: outcome.sha256,
        })
    }
}

impl Default for AudioOnlyDownloader {
    fn default() -> Self {
        Self::new().expect("default audio-only download clients")
    }
}

#[cfg(test)]
mod tests {
    use std::{fs, path::Path};

    use sha2::{Digest, Sha256};
    use tempfile::tempdir;

    use super::{AudioOnlyDownloader, AudioOutputPlan, AudioSourceKind};
    use crate::{
        dash::{DashMediaSegment, DashSegmentPlan, DashSegmentResource},
        dash_download::{DashTrackKind, DashTrackRequest},
        domain::{DownloadKind, HttpUrl, JobId, JobMetadata, Priority, UnixMillis},
        media::parse_hls_media_playlist,
        media_mux::AudioCodec,
        media_worker::FfmpegContainer,
        state::DownloadState,
        storage::Database,
        test_support::{Fixture, FixtureServer},
    };
    use url::Url;

    fn timestamp(value: i64) -> UnixMillis {
        UnixMillis::new(value).expect("timestamp")
    }

    fn audio_job(id: JobId, source_url: HttpUrl, destination: &Path) -> JobMetadata {
        JobMetadata {
            id,
            display_url: source_url.as_str().to_owned(),
            source_url,
            final_url: None,
            filename: "audio.m4a".to_owned(),
            destination: destination.to_owned(),
            state: DownloadState::Downloading,
            kind: DownloadKind::Media,
            total_bytes: None,
            downloaded_bytes: 0,
            category: None,
            queue: None,
            priority: Priority::Normal,
            source_browser: None,
            source_profile: None,
            retry_count: 0,
            error: None,
            created_at: timestamp(1),
            updated_at: timestamp(1),
            completed_at: None,
        }
    }

    fn resource(url: &str) -> DashSegmentResource {
        DashSegmentResource {
            url: Url::parse(url).expect("resource URL"),
            byte_range: None,
        }
    }

    #[test]
    fn output_plans_preserve_direct_types_and_select_copy_only_segmented_containers() {
        let direct = AudioOutputPlan::direct(AudioCodec::Mp3, ".MP3").expect("direct plan");
        assert_eq!(direct.source, AudioSourceKind::Direct);
        assert_eq!(direct.extension, "mp3");
        assert_eq!(direct.container, None);

        for (source, codec, container, extension) in [
            (
                AudioSourceKind::Hls,
                AudioCodec::Aac,
                FfmpegContainer::Mp4,
                "m4a",
            ),
            (
                AudioSourceKind::Dash,
                AudioCodec::Opus,
                FfmpegContainer::WebM,
                "weba",
            ),
            (
                AudioSourceKind::Dash,
                AudioCodec::Flac,
                FfmpegContainer::Matroska,
                "mka",
            ),
        ] {
            let plan = AudioOutputPlan::segmented(source, codec).expect("segmented plan");
            assert_eq!(plan.container, Some(container));
            assert_eq!(plan.extension, extension);
        }
    }

    #[test]
    fn direct_audio_fixture_preserves_exact_bytes_and_hash() {
        let bytes = b"ID3\x04\0\0progressive-audio";
        let server = FixtureServer::start(vec![Fixture::ok(bytes.as_slice())]);
        let directory = tempdir().expect("tempdir");
        let output = directory.path().join("direct.mp3");
        let url = HttpUrl::parse(server.url("/audio.mp3")).expect("audio URL");

        let outcome = AudioOnlyDownloader::new()
            .expect("downloader")
            .download_direct(&url, &output)
            .expect("direct audio");

        assert_eq!(outcome.source, AudioSourceKind::Direct);
        assert_eq!(outcome.downloaded_units, 1);
        assert_eq!(fs::read(output).expect("output"), bytes);
        assert_eq!(outcome.sha256, format!("{:x}", Sha256::digest(bytes)));
    }

    #[test]
    fn hls_audio_fixture_writes_init_and_fragments_in_order() {
        let server = FixtureServer::start(vec![
            Fixture::ok("AUDIO-INIT"),
            Fixture::ok("AUDIO-ONE"),
            Fixture::ok("AUDIO-TWO"),
        ]);
        let playlist = parse_hls_media_playlist(
            "#EXTM3U\n#EXT-X-TARGETDURATION:4\n#EXT-X-MAP:URI=\"init.mp4\"\n\
             #EXTINF:4,\none.m4s\n#EXTINF:4,\ntwo.m4s\n#EXT-X-ENDLIST",
            &HttpUrl::parse(server.url("/audio.m3u8")).expect("playlist URL"),
        )
        .expect("audio playlist");
        let directory = tempdir().expect("tempdir");
        let output = directory.path().join("audio.m4a.part");

        let outcome = AudioOnlyDownloader::new()
            .expect("downloader")
            .download_hls(&playlist, &output)
            .expect("HLS audio");

        let expected = b"AUDIO-INITAUDIO-ONEAUDIO-TWO";
        assert_eq!(outcome.source, AudioSourceKind::Hls);
        assert_eq!(outcome.downloaded_units, 3);
        assert_eq!(fs::read(output).expect("output"), expected);
        assert_eq!(outcome.sha256, format!("{:x}", Sha256::digest(expected)));
    }

    #[test]
    fn dash_audio_fixture_requests_only_the_selected_audio_representation() {
        let audio_server = FixtureServer::start(vec![
            Fixture::ok("AUDIO-INIT"),
            Fixture::ok("AUDIO-ONE"),
            Fixture::ok("AUDIO-TWO"),
        ]);
        let unselected_video = FixtureServer::start(vec![Fixture::ok("VIDEO")]);
        let directory = tempdir().expect("tempdir");
        let database_path = directory.path().join("state.sqlite");
        let output = directory.path().join("audio.m4a.part");
        let job_id = JobId::new();
        let source_url = HttpUrl::parse(audio_server.url("/manifest.mpd")).expect("source URL");
        let mut database = Database::open(&database_path).expect("database");
        database
            .create_job(&audio_job(job_id, source_url, &output), None)
            .expect("audio job");
        drop(database);
        let plan = DashSegmentPlan {
            initialization: Some(resource(&audio_server.url("/audio-init"))),
            media: vec![
                DashMediaSegment {
                    number: 1,
                    time: 0,
                    duration: 4,
                    resource: resource(&audio_server.url("/audio-1")),
                },
                DashMediaSegment {
                    number: 2,
                    time: 4,
                    duration: 4,
                    resource: resource(&audio_server.url("/audio-2")),
                },
            ],
        };

        let outcome = AudioOnlyDownloader::new()
            .expect("downloader")
            .resume_dash(
                &database_path,
                DashTrackRequest {
                    kind: DashTrackKind::Audio,
                    job_id,
                    plan,
                    temporary_path: output.clone(),
                },
                timestamp(2),
            )
            .expect("DASH audio");

        let expected = b"AUDIO-INITAUDIO-ONEAUDIO-TWO";
        assert_eq!(outcome.source, AudioSourceKind::Dash);
        assert_eq!(outcome.downloaded_units, 3);
        assert_eq!(fs::read(output).expect("output"), expected);
        assert_eq!(outcome.sha256, format!("{:x}", Sha256::digest(expected)));
        assert_eq!(audio_server.requests().len(), 3);
        assert_eq!(unselected_video.requests().len(), 0);
    }
}
