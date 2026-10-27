//! Bounded subtitle download and safe plain-text SRT conversion.

use std::{
    collections::BTreeMap,
    fs::{self, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
};

use quick_xml::{Reader, events::Event};
use thiserror::Error;
use uuid::Uuid;

use crate::{
    domain::HttpUrl,
    transfer::{SingleStreamDownloader, TransferControl, TransferError, TransferStatus},
};

/// Maximum accepted source document size. Subtitle conversion is intentionally memory-bounded.
pub const MAX_SUBTITLE_SOURCE_BYTES: usize = 8 * 1024 * 1024;
const MAX_CUES: usize = 100_000;
const MAX_XML_DEPTH: usize = 64;
const MAX_XML_NODES: usize = 500_000;
const MAX_CUE_TEXT_BYTES: usize = 64 * 1024;

/// A text subtitle format with a deterministic conversion path to SRT.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubtitleFormat {
    WebVtt,
    Ttml,
}

/// A validated SRT document and its cue count.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubtitleConversion {
    pub contents: String,
    pub cue_count: usize,
}

/// Download and conversion evidence for one selected subtitle track.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubtitleDownloadOutcome {
    pub downloaded_bytes: u64,
    pub source_sha256: String,
    pub output_bytes: u64,
    pub cue_count: usize,
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum SubtitleConversionError {
    #[error("subtitle source exceeds the {MAX_SUBTITLE_SOURCE_BYTES}-byte limit")]
    SourceTooLarge,
    #[error("subtitle source is not valid UTF-8")]
    InvalidUtf8,
    #[error("invalid WebVTT: {0}")]
    InvalidWebVtt(String),
    #[error("invalid TTML: {0}")]
    InvalidTtml(String),
    #[error("subtitle document exceeds the supported complexity limit")]
    ComplexityLimit,
    #[error("subtitle cue has unsupported or ambiguous timing: {0}")]
    UnsupportedTiming(String),
    #[error("subtitle cue contains unsupported markup or text: {0}")]
    UnsupportedText(String),
    #[error("subtitle document does not contain any cues")]
    NoCues,
}

#[derive(Debug, Error)]
pub enum SubtitleDownloadError {
    #[error("subtitle download failed: {0}")]
    Transfer(#[from] TransferError),
    #[error("subtitle conversion failed: {0}")]
    Conversion(#[from] SubtitleConversionError),
    #[error("subtitle output already exists: {0}")]
    OutputExists(PathBuf),
    #[error("subtitle filesystem operation failed: {0}")]
    Io(#[from] io::Error),
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Cue {
    start_ms: u64,
    end_ms: u64,
    text: String,
}

/// Converts a complete UTF-8 `WebVTT` or TTML document to plain-text SRT.
///
/// Styling and positioning metadata are deliberately discarded. Timing must be finite,
/// millisecond-exact, and have a strictly positive duration; TTML frame/tick timing and timed
/// ancestors or spans are rejected rather than approximated.
///
/// # Errors
/// Returns an error for oversized, malformed, ambiguous, or cue-less input.
pub fn convert_to_srt(
    source: &[u8],
    format: SubtitleFormat,
) -> Result<SubtitleConversion, SubtitleConversionError> {
    if source.len() > MAX_SUBTITLE_SOURCE_BYTES {
        return Err(SubtitleConversionError::SourceTooLarge);
    }
    let document = std::str::from_utf8(source)
        .map_err(|_| SubtitleConversionError::InvalidUtf8)?
        .trim_start_matches('\u{feff}');
    let cues = match format {
        SubtitleFormat::WebVtt => parse_webvtt(document)?,
        SubtitleFormat::Ttml => parse_ttml(document)?,
    };
    if cues.is_empty() {
        return Err(SubtitleConversionError::NoCues);
    }
    let mut contents = String::new();
    for (index, cue) in cues.iter().enumerate() {
        use std::fmt::Write as _;
        writeln!(contents, "{}", index + 1).expect("writing to a string cannot fail");
        writeln!(
            contents,
            "{} --> {}",
            srt_timestamp(cue.start_ms),
            srt_timestamp(cue.end_ms)
        )
        .expect("writing to a string cannot fail");
        contents.push_str(&cue.text);
        contents.push_str("\n\n");
    }
    Ok(SubtitleConversion {
        contents,
        cue_count: cues.len(),
    })
}

/// Downloads one explicitly selected subtitle resource and converts it to a new SRT file.
///
/// The remote source is held in a unique sidecar only while it is validated. Malformed input,
/// transfer failures, and output-write failures remove intermediates and incomplete output.
#[derive(Debug, Clone)]
pub struct SubtitleDownloader {
    transfer: SingleStreamDownloader,
}

impl SubtitleDownloader {
    /// Creates a bounded HTTP(S) subtitle client.
    ///
    /// # Errors
    /// Returns an error if the underlying client cannot be initialized.
    pub fn new() -> Result<Self, SubtitleDownloadError> {
        Ok(Self {
            transfer: SingleStreamDownloader::new()?,
        })
    }

    /// Downloads and safely converts the selected track without overwriting an existing output.
    ///
    /// # Errors
    /// Returns an HTTP, size, syntax, timing, or filesystem error.
    pub fn download_as_srt(
        &self,
        url: &HttpUrl,
        format: SubtitleFormat,
        output: &Path,
    ) -> Result<SubtitleDownloadOutcome, SubtitleDownloadError> {
        if output.exists() {
            return Err(SubtitleDownloadError::OutputExists(output.to_owned()));
        }
        if let Some(parent) = output.parent()
            && !parent.as_os_str().is_empty()
        {
            fs::create_dir_all(parent)?;
        }
        let temporary = temporary_source_path(output);
        let control = TransferControl::default();
        let callback_control = control.clone();
        let transfer =
            self.transfer
                .download_with_control(url, &temporary, &control, move |progress| {
                    if progress.downloaded_bytes > MAX_SUBTITLE_SOURCE_BYTES as u64 {
                        callback_control.request_pause();
                    }
                });
        let transfer = match transfer {
            Ok(TransferStatus::Completed(outcome))
                if outcome.downloaded_bytes <= MAX_SUBTITLE_SOURCE_BYTES as u64 =>
            {
                outcome
            }
            Ok(TransferStatus::Completed(_) | TransferStatus::Paused { .. }) => {
                remove_if_present(&temporary);
                return Err(SubtitleConversionError::SourceTooLarge.into());
            }
            Err(error) => {
                remove_if_present(&temporary);
                return Err(error.into());
            }
        };
        let source = match fs::read(&temporary) {
            Ok(source) => source,
            Err(error) => {
                remove_if_present(&temporary);
                return Err(error.into());
            }
        };
        let conversion = match convert_to_srt(&source, format) {
            Ok(conversion) => conversion,
            Err(error) => {
                remove_if_present(&temporary);
                return Err(error.into());
            }
        };
        remove_if_present(&temporary);

        let output_bytes = u64::try_from(conversion.contents.len())
            .map_err(|_| io::Error::other("subtitle output length does not fit u64"))?;
        if let Err(error) = write_new_synced(output, conversion.contents.as_bytes()) {
            return Err(if error.kind() == io::ErrorKind::AlreadyExists {
                SubtitleDownloadError::OutputExists(output.to_owned())
            } else {
                error.into()
            });
        }
        Ok(SubtitleDownloadOutcome {
            downloaded_bytes: transfer.downloaded_bytes,
            source_sha256: transfer.sha256,
            output_bytes,
            cue_count: conversion.cue_count,
        })
    }
}

impl Default for SubtitleDownloader {
    fn default() -> Self {
        Self::new().expect("default subtitle download client")
    }
}

fn parse_webvtt(document: &str) -> Result<Vec<Cue>, SubtitleConversionError> {
    let normalized = document.replace("\r\n", "\n").replace('\r', "\n");
    let lines: Vec<_> = normalized.lines().collect();
    let Some(header) = lines.first() else {
        return Err(invalid_vtt("missing WEBVTT header"));
    };
    if *header != "WEBVTT" && !header.starts_with("WEBVTT ") && !header.starts_with("WEBVTT\t") {
        return Err(invalid_vtt("missing WEBVTT header"));
    }
    let body_start = lines
        .iter()
        .position(|line| line.trim().is_empty())
        .map(|index| index + 1)
        .ok_or_else(|| invalid_vtt("header must end with a blank line"))?;
    let mut cues = Vec::new();
    let mut cursor = body_start;
    while cursor < lines.len() {
        while cursor < lines.len() && lines[cursor].trim().is_empty() {
            cursor += 1;
        }
        let start = cursor;
        while cursor < lines.len() && !lines[cursor].trim().is_empty() {
            cursor += 1;
        }
        if start == cursor {
            continue;
        }
        let block = &lines[start..cursor];
        if block[0] == "NOTE"
            || block[0].starts_with("NOTE ")
            || matches!(block[0], "STYLE" | "REGION")
        {
            continue;
        }
        if cues.len() >= MAX_CUES {
            return Err(SubtitleConversionError::ComplexityLimit);
        }
        let timing_index = if block[0].contains("-->") {
            0
        } else if block.get(1).is_some_and(|line| line.contains("-->")) {
            1
        } else {
            return Err(invalid_vtt("cue is missing a timing line"));
        };
        if block.len() <= timing_index + 1 {
            return Err(invalid_vtt("cue text is empty"));
        }
        let (start_ms, end_ms) = parse_vtt_timing(block[timing_index])?;
        let text = plain_vtt_text(&block[timing_index + 1..].join("\n"))?;
        validate_cue(start_ms, end_ms, &text)?;
        cues.push(Cue {
            start_ms,
            end_ms,
            text,
        });
    }
    Ok(cues)
}

fn parse_vtt_timing(line: &str) -> Result<(u64, u64), SubtitleConversionError> {
    let (start, remainder) = line
        .split_once("-->")
        .ok_or_else(|| invalid_vtt("malformed cue timing"))?;
    let start = start.trim();
    let end = remainder
        .split_ascii_whitespace()
        .next()
        .ok_or_else(|| invalid_vtt("missing cue end time"))?;
    if start.is_empty() || remainder.matches("-->").count() != 0 {
        return Err(invalid_vtt("malformed cue timing"));
    }
    Ok((parse_vtt_timestamp(start)?, parse_vtt_timestamp(end)?))
}

fn parse_vtt_timestamp(value: &str) -> Result<u64, SubtitleConversionError> {
    let parts: Vec<_> = value.split(':').collect();
    let (hours, minutes, seconds) = match parts.as_slice() {
        [minutes, seconds] => (0, parse_digits(minutes)?, parse_second_millis(seconds)?),
        [hours, minutes, seconds] => (
            parse_digits(hours)?,
            parse_digits(minutes)?,
            parse_second_millis(seconds)?,
        ),
        _ => return Err(unsupported_timing(value)),
    };
    if minutes >= 60 || seconds.0 >= 60 {
        return Err(unsupported_timing(value));
    }
    milliseconds(hours, minutes, seconds.0, seconds.1).ok_or_else(|| unsupported_timing(value))
}

fn plain_vtt_text(input: &str) -> Result<String, SubtitleConversionError> {
    let mut output = String::new();
    let mut remaining = input;
    while let Some(index) = remaining.find(['<', '&']) {
        output.push_str(&remaining[..index]);
        remaining = &remaining[index..];
        if remaining.starts_with('<') {
            let end = remaining
                .find('>')
                .ok_or_else(|| unsupported_text("unterminated WebVTT tag"))?;
            let tag = remaining[1..end].trim().to_ascii_lowercase();
            if matches!(tag.as_str(), "br" | "br/" | "br /") {
                output.push('\n');
            }
            remaining = &remaining[end + 1..];
        } else {
            let end = remaining
                .find(';')
                .ok_or_else(|| unsupported_text("unterminated WebVTT entity"))?;
            output.push(decode_entity(&remaining[1..end])?);
            remaining = &remaining[end + 1..];
        }
    }
    output.push_str(remaining);
    clean_text(output)
}

#[derive(Debug)]
struct TtmlCue {
    start_ms: u64,
    end_ms: u64,
    text: String,
    depth: usize,
    preserve_space: bool,
}

fn parse_ttml(document: &str) -> Result<Vec<Cue>, SubtitleConversionError> {
    let mut reader = Reader::from_str(document);
    reader.config_mut().trim_text(false);
    let mut depth = 0_usize;
    let mut nodes = 0_usize;
    let mut seen_root = false;
    let mut body_depth = None;
    let mut current: Option<TtmlCue> = None;
    let mut cues = Vec::new();

    loop {
        match reader.read_event() {
            Ok(Event::Start(element)) => {
                nodes += 1;
                check_xml_complexity(nodes, depth)?;
                let name = xml_local_name(element.name().as_ref())?;
                let attributes = xml_attributes(&reader, &element)?;
                if depth == 0 {
                    if seen_root || name != "tt" {
                        return Err(invalid_ttml("root element must be tt"));
                    }
                    seen_root = true;
                }
                if name == "body" {
                    if body_depth.is_some() {
                        return Err(invalid_ttml("multiple body elements"));
                    }
                    body_depth = Some(depth);
                }
                handle_ttml_start(&name, &attributes, depth, body_depth, &mut current)?;
                depth += 1;
            }
            Ok(Event::Empty(element)) => {
                nodes += 1;
                check_xml_complexity(nodes, depth)?;
                let name = xml_local_name(element.name().as_ref())?;
                let attributes = xml_attributes(&reader, &element)?;
                reject_timed_non_cue(&name, &attributes)?;
                if name == "br" && current.is_some() {
                    push_ttml_text(&mut current, "\n")?;
                } else if name == "p" {
                    return Err(invalid_ttml("empty cue"));
                }
            }
            Ok(Event::Text(text)) => {
                if current.is_some() {
                    let decoded = text
                        .xml_content()
                        .map_err(|error| invalid_ttml(error.to_string()))?;
                    push_ttml_text(&mut current, &decoded)?;
                }
            }
            Ok(Event::CData(text)) => {
                if current.is_some() {
                    let decoded = text
                        .decode()
                        .map_err(|error| invalid_ttml(error.to_string()))?;
                    push_ttml_text(&mut current, &decoded)?;
                }
            }
            Ok(Event::GeneralRef(reference)) => {
                push_ttml_reference(&reference, &mut current)?;
            }
            Ok(Event::End(element)) => {
                if depth == 0 {
                    return Err(invalid_ttml("unexpected closing element"));
                }
                depth -= 1;
                let name = xml_local_name(element.name().as_ref())?;
                if current
                    .as_ref()
                    .is_some_and(|cue| cue.depth == depth && name == "p")
                {
                    let cue = current.take().expect("checked current cue");
                    let text = if cue.preserve_space {
                        clean_text(cue.text.trim().to_owned())?
                    } else {
                        collapse_ttml_text(&cue.text)?
                    };
                    validate_cue(cue.start_ms, cue.end_ms, &text)?;
                    cues.push(Cue {
                        start_ms: cue.start_ms,
                        end_ms: cue.end_ms,
                        text,
                    });
                    if cues.len() > MAX_CUES {
                        return Err(SubtitleConversionError::ComplexityLimit);
                    }
                }
                if body_depth == Some(depth) && name == "body" {
                    body_depth = None;
                }
            }
            Ok(Event::DocType(_)) => {
                return Err(invalid_ttml("document types are not accepted"));
            }
            Ok(Event::Decl(_) | Event::Comment(_) | Event::PI(_)) => {}
            Ok(Event::Eof) => break,
            Err(error) => return Err(invalid_ttml(error.to_string())),
        }
    }
    finish_ttml(cues, seen_root, depth, current.as_ref())
}

fn finish_ttml(
    cues: Vec<Cue>,
    seen_root: bool,
    depth: usize,
    current: Option<&TtmlCue>,
) -> Result<Vec<Cue>, SubtitleConversionError> {
    if !seen_root || depth != 0 || current.is_some() {
        return Err(invalid_ttml("incomplete XML document"));
    }
    Ok(cues)
}

fn handle_ttml_start(
    name: &str,
    attributes: &BTreeMap<String, String>,
    depth: usize,
    body_depth: Option<usize>,
    current: &mut Option<TtmlCue>,
) -> Result<(), SubtitleConversionError> {
    if name == "p" {
        if body_depth.is_none() || current.is_some() {
            return Err(invalid_ttml(
                "cue must be a non-nested child of body content",
            ));
        }
        let begin = attributes
            .get("begin")
            .ok_or_else(|| unsupported_timing("TTML cue without begin"))
            .and_then(|value| parse_ttml_time(value))?;
        let end = match (attributes.get("end"), attributes.get("dur")) {
            (Some(end), None) => parse_ttml_time(end)?,
            (None, Some(duration)) => begin
                .checked_add(parse_ttml_time(duration)?)
                .ok_or_else(|| unsupported_timing(duration))?,
            _ => {
                return Err(unsupported_timing(
                    "TTML cue requires exactly one of end or dur",
                ));
            }
        };
        let preserve_space = attributes
            .get("space")
            .is_some_and(|value| value == "preserve");
        *current = Some(TtmlCue {
            start_ms: begin,
            end_ms: end,
            text: String::new(),
            depth,
            preserve_space,
        });
        return Ok(());
    }
    reject_timed_non_cue(name, attributes)?;
    if name == "br" && current.is_some() {
        push_ttml_text(current, "\n")?;
    }
    Ok(())
}

fn push_ttml_reference(
    reference: &quick_xml::events::BytesRef<'_>,
    current: &mut Option<TtmlCue>,
) -> Result<(), SubtitleConversionError> {
    if current.is_none() {
        return Ok(());
    }
    let character = if let Some(character) = reference
        .resolve_char_ref()
        .map_err(|error| invalid_ttml(error.to_string()))?
    {
        character
    } else {
        let name = reference
            .decode()
            .map_err(|error| invalid_ttml(error.to_string()))?;
        decode_entity(&name)?
    };
    let mut buffer = [0_u8; 4];
    push_ttml_text(current, character.encode_utf8(&mut buffer))
}

fn reject_timed_non_cue(
    name: &str,
    attributes: &BTreeMap<String, String>,
) -> Result<(), SubtitleConversionError> {
    if name != "p"
        && ["begin", "end", "dur"]
            .iter()
            .any(|key| attributes.contains_key(*key))
    {
        return Err(unsupported_timing(format!("timed TTML {name} element")));
    }
    Ok(())
}

fn parse_ttml_time(value: &str) -> Result<u64, SubtitleConversionError> {
    if let Some(milliseconds) = value.strip_suffix("ms") {
        return parse_digits(milliseconds).map_err(|_| unsupported_timing(value));
    }
    if let Some(seconds) = value.strip_suffix('s') {
        let (whole, fraction) = parse_decimal_milliseconds(seconds)?;
        return whole
            .checked_mul(1_000)
            .and_then(|base| base.checked_add(fraction))
            .ok_or_else(|| unsupported_timing(value));
    }
    let parts: Vec<_> = value.split(':').collect();
    let [hours, minutes, seconds] = parts.as_slice() else {
        return Err(unsupported_timing(value));
    };
    let hours = parse_digits(hours)?;
    let minutes = parse_digits(minutes)?;
    let (seconds, fraction) = parse_second_millis(seconds)?;
    if minutes >= 60 || seconds >= 60 {
        return Err(unsupported_timing(value));
    }
    milliseconds(hours, minutes, seconds, fraction).ok_or_else(|| unsupported_timing(value))
}

fn parse_decimal_milliseconds(value: &str) -> Result<(u64, u64), SubtitleConversionError> {
    let (whole, fraction) = value.split_once('.').unwrap_or((value, ""));
    let whole = parse_digits(whole)?;
    let fraction = match fraction.len() {
        0 => 0,
        1 => parse_digits(fraction)? * 100,
        2 => parse_digits(fraction)? * 10,
        3 => parse_digits(fraction)?,
        _ => return Err(unsupported_timing(value)),
    };
    Ok((whole, fraction))
}

fn parse_second_millis(value: &str) -> Result<(u64, u64), SubtitleConversionError> {
    let (seconds, fraction) = value
        .split_once('.')
        .ok_or_else(|| unsupported_timing(value))?;
    if fraction.len() != 3 {
        return Err(unsupported_timing(value));
    }
    Ok((parse_digits(seconds)?, parse_digits(fraction)?))
}

fn parse_digits(value: &str) -> Result<u64, SubtitleConversionError> {
    if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(unsupported_timing(value));
    }
    value.parse().map_err(|_| unsupported_timing(value))
}

fn milliseconds(hours: u64, minutes: u64, seconds: u64, millis: u64) -> Option<u64> {
    hours
        .checked_mul(60)?
        .checked_add(minutes)?
        .checked_mul(60)?
        .checked_add(seconds)?
        .checked_mul(1_000)?
        .checked_add(millis)
}

fn srt_timestamp(milliseconds: u64) -> String {
    let millis = milliseconds % 1_000;
    let total_seconds = milliseconds / 1_000;
    let seconds = total_seconds % 60;
    let total_minutes = total_seconds / 60;
    let minutes = total_minutes % 60;
    let hours = total_minutes / 60;
    format!("{hours:02}:{minutes:02}:{seconds:02},{millis:03}")
}

fn collapse_ttml_text(input: &str) -> Result<String, SubtitleConversionError> {
    let text = input
        .split('\n')
        .map(|line| line.split_whitespace().collect::<Vec<_>>().join(" "))
        .collect::<Vec<_>>()
        .join("\n")
        .trim_matches('\n')
        .to_owned();
    clean_text(text)
}

fn clean_text(text: String) -> Result<String, SubtitleConversionError> {
    if text.len() > MAX_CUE_TEXT_BYTES {
        return Err(SubtitleConversionError::ComplexityLimit);
    }
    if text.is_empty() {
        return Err(unsupported_text("empty cue text"));
    }
    if text.chars().any(|character| {
        character == '\0' || (character.is_control() && !matches!(character, '\n' | '\t'))
    }) {
        return Err(unsupported_text("control character"));
    }
    Ok(text)
}

fn validate_cue(start_ms: u64, end_ms: u64, text: &str) -> Result<(), SubtitleConversionError> {
    if end_ms <= start_ms {
        return Err(unsupported_timing("cue end must be after cue start"));
    }
    if text.is_empty() {
        return Err(unsupported_text("empty cue text"));
    }
    Ok(())
}

fn decode_entity(name: &str) -> Result<char, SubtitleConversionError> {
    let character = match name {
        "lt" => '<',
        "gt" => '>',
        "amp" => '&',
        "apos" => '\'',
        "quot" => '"',
        "nbsp" => '\u{a0}',
        "lrm" => '\u{200e}',
        "rlm" => '\u{200f}',
        value if value.starts_with("#x") => u32::from_str_radix(&value[2..], 16)
            .ok()
            .and_then(char::from_u32)
            .ok_or_else(|| unsupported_text("invalid character reference"))?,
        value if value.starts_with('#') => value[1..]
            .parse::<u32>()
            .ok()
            .and_then(char::from_u32)
            .ok_or_else(|| unsupported_text("invalid character reference"))?,
        _ => return Err(unsupported_text(format!("unknown entity &{name};"))),
    };
    if character == '\0' || (character.is_control() && !matches!(character, '\n' | '\t')) {
        return Err(unsupported_text("control character reference"));
    }
    Ok(character)
}

fn xml_attributes(
    reader: &Reader<&[u8]>,
    element: &quick_xml::events::BytesStart<'_>,
) -> Result<BTreeMap<String, String>, SubtitleConversionError> {
    let mut attributes = BTreeMap::new();
    for attribute in element.attributes() {
        let attribute = attribute.map_err(|error| invalid_ttml(error.to_string()))?;
        let key = xml_local_name(attribute.key.as_ref())?;
        let value = attribute
            .decode_and_unescape_value(reader.decoder())
            .map_err(|error| invalid_ttml(error.to_string()))?
            .into_owned();
        if attributes.insert(key, value).is_some() {
            return Err(invalid_ttml("duplicate local attribute name"));
        }
    }
    Ok(attributes)
}

fn xml_local_name(name: &[u8]) -> Result<String, SubtitleConversionError> {
    let local = name.rsplit(|byte| *byte == b':').next().unwrap_or(name);
    std::str::from_utf8(local)
        .map(str::to_owned)
        .map_err(|error| invalid_ttml(error.to_string()))
}

fn check_xml_complexity(nodes: usize, depth: usize) -> Result<(), SubtitleConversionError> {
    if nodes > MAX_XML_NODES || depth >= MAX_XML_DEPTH {
        return Err(SubtitleConversionError::ComplexityLimit);
    }
    Ok(())
}

fn push_ttml_text(
    current: &mut Option<TtmlCue>,
    text: &str,
) -> Result<(), SubtitleConversionError> {
    if let Some(cue) = current {
        if cue.text.len().saturating_add(text.len()) > MAX_CUE_TEXT_BYTES {
            return Err(SubtitleConversionError::ComplexityLimit);
        }
        cue.text.push_str(text);
    }
    Ok(())
}

fn temporary_source_path(output: &Path) -> PathBuf {
    let name = output
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("subtitle.srt");
    output.with_file_name(format!(".{name}.{}.source.part", Uuid::new_v4()))
}

fn write_new_synced(path: &Path, contents: &[u8]) -> io::Result<()> {
    let mut output = OpenOptions::new().write(true).create_new(true).open(path)?;
    let result = output
        .write_all(contents)
        .and_then(|()| output.flush())
        .and_then(|()| output.sync_all());
    if result.is_err() {
        drop(output);
        remove_if_present(path);
    }
    result
}

fn remove_if_present(path: &Path) {
    let _ = fs::remove_file(path);
}

fn invalid_vtt(message: impl Into<String>) -> SubtitleConversionError {
    SubtitleConversionError::InvalidWebVtt(message.into())
}

fn invalid_ttml(message: impl Into<String>) -> SubtitleConversionError {
    SubtitleConversionError::InvalidTtml(message.into())
}

fn unsupported_timing(message: impl Into<String>) -> SubtitleConversionError {
    SubtitleConversionError::UnsupportedTiming(message.into())
}

fn unsupported_text(message: impl Into<String>) -> SubtitleConversionError {
    SubtitleConversionError::UnsupportedText(message.into())
}

#[cfg(test)]
mod tests {
    use std::fs;

    use sha2::{Digest, Sha256};
    use tempfile::tempdir;

    use super::{
        MAX_SUBTITLE_SOURCE_BYTES, SubtitleConversionError, SubtitleDownloadError,
        SubtitleDownloader, SubtitleFormat, convert_to_srt,
    };
    use crate::{
        domain::HttpUrl,
        test_support::{Fixture, FixtureServer},
    };

    #[test]
    fn webvtt_timing_unicode_and_markup_convert_to_plain_srt() {
        let source = "\u{feff}WEBVTT বাংলা\nKind: captions\n\nintro\n00:01.250 --> 00:03.500 align:start\n<v Narrator><b>স্বাগতম</b> &amp; café</v>\n\n01:02:03.004 --> 01:02:05.006\n第二行<br>line two\n";

        let converted =
            convert_to_srt(source.as_bytes(), SubtitleFormat::WebVtt).expect("WebVTT conversion");

        assert_eq!(converted.cue_count, 2);
        assert_eq!(
            converted.contents,
            "1\n00:00:01,250 --> 00:00:03,500\nস্বাগতম & café\n\n\
             2\n01:02:03,004 --> 01:02:05,006\n第二行\nline two\n\n"
        );
    }

    #[test]
    fn ttml_clock_offset_duration_nested_text_and_unicode_convert_to_srt() {
        let source = r#"<?xml version="1.0" encoding="UTF-8"?>
          <tt xmlns="http://www.w3.org/ns/ttml" xml:lang="bn">
            <body><div>
              <p begin="00:00:01.125" end="00:00:03.500">হ্যালো <span>বিশ্ব</span><br/>café &amp; 東京</p>
              <p begin="4.5s" dur="1250ms">শেষ</p>
            </div></body>
          </tt>"#;

        let converted =
            convert_to_srt(source.as_bytes(), SubtitleFormat::Ttml).expect("TTML conversion");

        assert_eq!(converted.cue_count, 2);
        assert_eq!(
            converted.contents,
            "1\n00:00:01,125 --> 00:00:03,500\nহ্যালো বিশ্ব\ncafé & 東京\n\n\
             2\n00:00:04,500 --> 00:00:05,750\nশেষ\n\n"
        );
    }

    #[test]
    fn malformed_and_ambiguous_documents_are_rejected() {
        let cases = [
            (
                SubtitleFormat::WebVtt,
                "WEBVTT\n\n00:00.000 --> 00:00.000\nzero",
            ),
            (
                SubtitleFormat::WebVtt,
                "WEBVTT\n\n00:61.000 --> 00:62.000\nbad minute",
            ),
            (
                SubtitleFormat::WebVtt,
                "WEBVTT\n\n00:00.000 --> 00:01.000\nunterminated <b",
            ),
            (
                SubtitleFormat::Ttml,
                "<tt><body begin='1s'><p begin='1s' end='2s'>timed parent</p></body></tt>",
            ),
            (
                SubtitleFormat::Ttml,
                "<tt><body><p begin='00:00:00:12' end='2s'>frames</p></body></tt>",
            ),
            (
                SubtitleFormat::Ttml,
                "<tt><body><p begin='1s' end='2s' dur='1s'>ambiguous</p></body></tt>",
            ),
            (
                SubtitleFormat::Ttml,
                "<!DOCTYPE tt [<!ENTITY x 'unsafe'>]><tt><body><p begin='1s' end='2s'>&x;</p></body></tt>",
            ),
        ];
        for (format, source) in cases {
            assert!(
                convert_to_srt(source.as_bytes(), format).is_err(),
                "accepted malformed source: {source}"
            );
        }
        assert_eq!(
            convert_to_srt(&[0xff, 0xfe], SubtitleFormat::WebVtt),
            Err(SubtitleConversionError::InvalidUtf8)
        );
        assert_eq!(
            convert_to_srt(
                &vec![b'x'; MAX_SUBTITLE_SOURCE_BYTES + 1],
                SubtitleFormat::WebVtt
            ),
            Err(SubtitleConversionError::SourceTooLarge)
        );
    }

    #[test]
    fn selected_webvtt_and_ttml_tracks_download_with_hashes_and_valid_srt() {
        let webvtt = b"WEBVTT\n\n00:00.000 --> 00:02.000\nDownloaded \xE2\x9C\x93\n";
        let ttml = b"<tt><body><p begin='2s' end='3.25s'>TTML \xE2\x9C\x93</p></body></tt>";
        let server = FixtureServer::start(vec![
            Fixture::ok(webvtt.as_slice()),
            Fixture::ok(ttml.as_slice()),
        ]);
        let directory = tempdir().expect("tempdir");
        let downloader = SubtitleDownloader::new().expect("downloader");
        let cases = [
            (
                webvtt.as_slice(),
                "/captions-en.vtt",
                SubtitleFormat::WebVtt,
                "webvtt.srt",
                "1\n00:00:00,000 --> 00:00:02,000\nDownloaded ✓\n\n",
            ),
            (
                ttml.as_slice(),
                "/captions-bn.ttml",
                SubtitleFormat::Ttml,
                "ttml.srt",
                "1\n00:00:02,000 --> 00:00:03,250\nTTML ✓\n\n",
            ),
        ];
        for (source, path, format, filename, expected) in cases {
            let output = directory.path().join(filename);
            let outcome = downloader
                .download_as_srt(
                    &HttpUrl::parse(server.url(path)).expect("subtitle URL"),
                    format,
                    &output,
                )
                .expect("subtitle output");

            assert_eq!(outcome.downloaded_bytes, source.len() as u64);
            assert_eq!(
                outcome.source_sha256,
                format!("{:x}", Sha256::digest(source))
            );
            assert_eq!(outcome.cue_count, 1);
            assert_eq!(fs::read_to_string(output).expect("SRT output"), expected);
        }
        assert_eq!(server.requests().len(), 2);
    }

    #[test]
    fn failed_conversion_leaves_no_output_or_source_sidecar() {
        let server = FixtureServer::start(vec![Fixture::ok("not WebVTT")]);
        let directory = tempdir().expect("tempdir");
        let output = directory.path().join("captions.srt");

        let result = SubtitleDownloader::new()
            .expect("downloader")
            .download_as_srt(
                &HttpUrl::parse(server.url("/broken.vtt")).expect("subtitle URL"),
                SubtitleFormat::WebVtt,
                &output,
            );

        assert!(matches!(result, Err(SubtitleDownloadError::Conversion(_))));
        assert!(!output.exists());
        assert_eq!(
            fs::read_dir(directory.path()).expect("directory").count(),
            0
        );
    }

    #[test]
    fn existing_output_is_preserved_without_requesting_the_track() {
        let server =
            FixtureServer::start(vec![Fixture::ok("WEBVTT\n\n00:00.000 --> 00:01.000\nnew")]);
        let directory = tempdir().expect("tempdir");
        let output = directory.path().join("captions.srt");
        fs::write(&output, "existing").expect("existing output");

        let result = SubtitleDownloader::new()
            .expect("downloader")
            .download_as_srt(
                &HttpUrl::parse(server.url("/captions.vtt")).expect("subtitle URL"),
                SubtitleFormat::WebVtt,
                &output,
            );

        assert!(matches!(
            result,
            Err(SubtitleDownloadError::OutputExists(_))
        ));
        assert_eq!(
            fs::read_to_string(output).expect("preserved output"),
            "existing"
        );
        assert_eq!(server.requests().len(), 0);
    }
}
