//! Bounded parsing of static MPEG-DASH adaptation and representation metadata.

use quick_xml::{Reader, events::Event};
use thiserror::Error;
use url::Url;

const MAX_MPD_BYTES: usize = 1_048_576;
const MAX_XML_DEPTH: usize = 32;
const MAX_XML_NODES: usize = 20_000;
const MAX_EXPANDED_SEGMENTS: usize = 100_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DashManifest {
    pub base_url: Url,
    pub periods: Vec<DashPeriod>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DashPeriod {
    pub id: Option<String>,
    pub base_url: Url,
    pub duration_millis: Option<u64>,
    pub adaptations: Vec<DashAdaptation>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DashAdaptation {
    pub id: Option<String>,
    pub content_type: Option<String>,
    pub mime_type: Option<String>,
    pub language: Option<String>,
    pub codecs: Option<String>,
    pub base_url: Url,
    pub content_protections: Vec<DashContentProtection>,
    pub representations: Vec<DashRepresentation>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DashRepresentation {
    pub id: Option<String>,
    pub bandwidth: Option<u64>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub frame_rate: Option<String>,
    pub mime_type: Option<String>,
    pub codecs: Option<String>,
    pub base_url: Url,
    pub content_protections: Vec<DashContentProtection>,
    pub segment_plan: Option<DashSegmentPlan>,
}

/// Fully resolved resources for one DASH representation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DashSegmentPlan {
    pub initialization: Option<DashSegmentResource>,
    pub media: Vec<DashMediaSegment>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DashSegmentResource {
    pub url: Url,
    pub byte_range: Option<DashByteRange>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DashByteRange {
    pub start: u64,
    pub end: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DashMediaSegment {
    pub number: u64,
    pub time: u64,
    pub duration: u64,
    pub resource: DashSegmentResource,
}

/// An uninterpreted DASH protection marker. Enforcement belongs to the protected-media policy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DashContentProtection {
    pub scheme_id_uri: Option<String>,
    pub value: Option<String>,
    pub default_kid: Option<String>,
    pub pssh: Option<String>,
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum DashParseError {
    #[error("MPD exceeds the 1048576-byte parser limit")]
    DocumentTooLarge,
    #[error("malformed MPD XML: {0}")]
    MalformedXml(String),
    #[error("document root must be MPD")]
    MissingMpd,
    #[error("dynamic DASH manifests are outside the static parser contract")]
    DynamicManifest,
    #[error("MPD exceeds the parser complexity limit")]
    ComplexityLimit,
    #[error("invalid BaseURL `{value}`")]
    InvalidBaseUrl { value: String },
    #[error("invalid numeric attribute {attribute}=`{value}`")]
    InvalidNumber { attribute: String, value: String },
    #[error("invalid ISO 8601 duration {attribute}=`{value}`")]
    InvalidDuration { attribute: String, value: String },
    #[error("malformed DASH segment metadata: {0}")]
    InvalidSegments(String),
    #[error("DASH segment expansion exceeds the 100000-segment limit")]
    SegmentLimit,
}

/// Parses adaptation sets and representations from a bounded static DASH MPD.
///
/// Relative `BaseURL` values are resolved at each MPD, Period, `AdaptationSet`, and
/// Representation level. Segment templates, timelines, and lists are inherited and expanded
/// into bounded, ordered per-representation plans.
///
/// # Errors
/// Returns an error for malformed or over-complex XML, dynamic manifests,
/// invalid inherited URLs, and malformed numeric representation attributes.
pub fn parse_static_mpd(
    document: &str,
    manifest_url: &Url,
) -> Result<DashManifest, DashParseError> {
    if document.len() > MAX_MPD_BYTES {
        return Err(DashParseError::DocumentTooLarge);
    }
    let root = parse_xml(document)?;
    if root.name != "MPD" {
        return Err(DashParseError::MissingMpd);
    }
    if root.attr("type").is_some_and(|kind| kind != "static") {
        return Err(DashParseError::DynamicManifest);
    }

    let base_url = resolve_base_url(&root, manifest_url)?;
    let manifest_duration = duration_attr_millis(&root, "mediaPresentationDuration")?;
    let period_nodes = root.children_named("Period").collect::<Vec<_>>();
    let period_durations = resolve_period_durations(&period_nodes, manifest_duration)?;
    let periods = period_nodes
        .into_iter()
        .zip(period_durations)
        .map(|(node, duration)| parse_period(node, &base_url, duration))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(DashManifest { base_url, periods })
}

fn parse_period(
    node: &XmlNode,
    parent_url: &Url,
    duration_millis: Option<u64>,
) -> Result<DashPeriod, DashParseError> {
    let base_url = resolve_base_url(node, parent_url)?;
    let adaptations = node
        .children_named("AdaptationSet")
        .map(|adaptation| parse_adaptation(adaptation, &base_url, duration_millis, node))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(DashPeriod {
        id: owned_attr(node, "id"),
        base_url,
        duration_millis,
        adaptations,
    })
}

fn resolve_period_durations(
    periods: &[&XmlNode],
    manifest_duration: Option<u64>,
) -> Result<Vec<Option<u64>>, DashParseError> {
    let mut starts = Vec::with_capacity(periods.len());
    let mut explicit_durations = Vec::with_capacity(periods.len());
    let mut cursor = Some(0_u64);
    for period in periods {
        let start = duration_attr_millis(period, "start")?.or(cursor);
        let duration = duration_attr_millis(period, "duration")?;
        cursor = start.and_then(|start| duration.and_then(|duration| start.checked_add(duration)));
        starts.push(start);
        explicit_durations.push(duration);
    }
    explicit_durations
        .iter()
        .enumerate()
        .map(|(index, explicit)| {
            if explicit.is_some() {
                return Ok(*explicit);
            }
            let start = starts[index];
            let end = starts
                .get(index + 1)
                .copied()
                .flatten()
                .or(manifest_duration);
            match (start, end) {
                (Some(start), Some(end)) if end >= start => Ok(Some(end - start)),
                (Some(_), Some(_)) => Err(invalid_segments("Period start times are not ordered")),
                _ => Ok(None),
            }
        })
        .collect()
}

fn parse_adaptation(
    node: &XmlNode,
    parent_url: &Url,
    period_duration_millis: Option<u64>,
    period_node: &XmlNode,
) -> Result<DashAdaptation, DashParseError> {
    let base_url = resolve_base_url(node, parent_url)?;
    let mime_type = owned_attr(node, "mimeType");
    let codecs = owned_attr(node, "codecs");
    let representations = node
        .children_named("Representation")
        .map(|representation| {
            parse_representation(
                representation,
                &base_url,
                mime_type.as_deref(),
                codecs.as_deref(),
                period_duration_millis,
                period_node,
                node,
            )
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(DashAdaptation {
        id: owned_attr(node, "id"),
        content_type: owned_attr(node, "contentType"),
        mime_type,
        language: owned_attr(node, "lang"),
        codecs,
        base_url,
        content_protections: parse_content_protections(node),
        representations,
    })
}

fn parse_representation(
    node: &XmlNode,
    parent_url: &Url,
    inherited_mime_type: Option<&str>,
    inherited_codecs: Option<&str>,
    period_duration_millis: Option<u64>,
    period_node: &XmlNode,
    adaptation_node: &XmlNode,
) -> Result<DashRepresentation, DashParseError> {
    let base_url = resolve_base_url(node, parent_url)?;
    Ok(DashRepresentation {
        id: owned_attr(node, "id"),
        bandwidth: number_attr(node, "bandwidth")?,
        width: number_attr(node, "width")?,
        height: number_attr(node, "height")?,
        frame_rate: owned_attr(node, "frameRate"),
        mime_type: owned_attr(node, "mimeType").or_else(|| inherited_mime_type.map(str::to_owned)),
        codecs: owned_attr(node, "codecs").or_else(|| inherited_codecs.map(str::to_owned)),
        segment_plan: parse_segment_plan(
            period_node,
            adaptation_node,
            node,
            &base_url,
            period_duration_millis,
        )?,
        base_url,
        content_protections: parse_content_protections(node),
    })
}

#[derive(Default)]
struct TemplateConfig<'a> {
    media: Option<&'a str>,
    initialization: Option<&'a str>,
    timescale: Option<u64>,
    duration: Option<u64>,
    start_number: Option<u64>,
    presentation_time_offset: Option<u64>,
    timeline: Option<&'a XmlNode>,
}

fn parse_segment_plan(
    period: &XmlNode,
    adaptation: &XmlNode,
    representation: &XmlNode,
    base_url: &Url,
    period_duration_millis: Option<u64>,
) -> Result<Option<DashSegmentPlan>, DashParseError> {
    let scopes = [period, adaptation, representation];
    let mut selected = None;
    for scope in scopes.iter().rev() {
        let template = scope.children_named("SegmentTemplate").next();
        let list = scope.children_named("SegmentList").next();
        if template.is_some() && list.is_some() {
            return Err(invalid_segments(
                "one scope cannot contain both SegmentTemplate and SegmentList",
            ));
        }
        if let Some(kind) = template
            .map(|_| "template")
            .or_else(|| list.map(|_| "list"))
        {
            selected = Some(kind);
            break;
        }
    }
    match selected {
        Some("template") => {
            expand_segment_template(&scopes, representation, base_url, period_duration_millis)
                .map(Some)
        }
        Some("list") => expand_segment_list(&scopes, base_url, period_duration_millis).map(Some),
        _ => Ok(None),
    }
}

fn expand_segment_template(
    scopes: &[&XmlNode; 3],
    representation: &XmlNode,
    base_url: &Url,
    period_duration_millis: Option<u64>,
) -> Result<DashSegmentPlan, DashParseError> {
    let mut config = TemplateConfig::default();
    for template in scopes
        .iter()
        .filter_map(|scope| scope.children_named("SegmentTemplate").next())
    {
        config.media = template.attr("media").or(config.media);
        config.initialization = template.attr("initialization").or(config.initialization);
        config.timescale = number_attr(template, "timescale")?.or(config.timescale);
        config.duration = number_attr(template, "duration")?.or(config.duration);
        config.start_number = number_attr(template, "startNumber")?.or(config.start_number);
        config.presentation_time_offset =
            number_attr(template, "presentationTimeOffset")?.or(config.presentation_time_offset);
        config.timeline = template
            .children_named("SegmentTimeline")
            .next()
            .or(config.timeline);
    }
    let media_template = config
        .media
        .ok_or_else(|| invalid_segments("SegmentTemplate requires media"))?;
    let timescale = config.timescale.unwrap_or(1);
    if timescale == 0 {
        return Err(invalid_segments(
            "SegmentTemplate timescale must be positive",
        ));
    }
    let presentation_time_offset = config.presentation_time_offset.unwrap_or(0);
    let spans = if let Some(timeline) = config.timeline {
        expand_timeline(
            timeline,
            period_duration_millis,
            timescale,
            presentation_time_offset,
        )?
    } else {
        expand_duration_template(config.duration, period_duration_millis, timescale)?
    };
    let start_number = config.start_number.unwrap_or(1);
    let representation_id = representation.attr("id");
    let bandwidth = number_attr::<u64>(representation, "bandwidth")?;
    let initialization = config
        .initialization
        .map(|template| {
            let path = substitute_template(
                template,
                representation_id,
                bandwidth,
                start_number,
                spans.first().map_or(0, |span| span.0),
            )?;
            Ok(DashSegmentResource {
                url: resolve_segment_url(base_url, &path)?,
                byte_range: None,
            })
        })
        .transpose()?;
    let media = spans
        .into_iter()
        .enumerate()
        .map(|(index, (time, duration))| {
            let number = start_number
                .checked_add(index as u64)
                .ok_or_else(|| invalid_segments("segment number overflow"))?;
            let path =
                substitute_template(media_template, representation_id, bandwidth, number, time)?;
            Ok(DashMediaSegment {
                number,
                time,
                duration,
                resource: DashSegmentResource {
                    url: resolve_segment_url(base_url, &path)?,
                    byte_range: None,
                },
            })
        })
        .collect::<Result<Vec<_>, DashParseError>>()?;
    Ok(DashSegmentPlan {
        initialization,
        media,
    })
}

fn expand_duration_template(
    duration: Option<u64>,
    period_duration_millis: Option<u64>,
    timescale: u64,
) -> Result<Vec<(u64, u64)>, DashParseError> {
    let duration = duration
        .filter(|duration| *duration > 0)
        .ok_or_else(|| invalid_segments("a timeline or positive duration is required"))?;
    let boundary = period_ticks(period_duration_millis, timescale)?
        .ok_or_else(|| invalid_segments("duration template requires a finite Period duration"))?;
    let count = boundary.div_ceil(duration);
    check_segment_count(count)?;
    (0..count)
        .map(|index| {
            index
                .checked_mul(duration)
                .map(|time| (time, duration))
                .ok_or_else(|| invalid_segments("segment time overflow"))
        })
        .collect()
}

fn expand_timeline(
    timeline: &XmlNode,
    period_duration_millis: Option<u64>,
    timescale: u64,
    presentation_time_offset: u64,
) -> Result<Vec<(u64, u64)>, DashParseError> {
    let entries = timeline.children_named("S").collect::<Vec<_>>();
    if entries.is_empty() {
        return Err(invalid_segments("SegmentTimeline must contain S entries"));
    }
    let period_end = period_ticks(period_duration_millis, timescale)?
        .and_then(|duration| presentation_time_offset.checked_add(duration));
    let mut spans = Vec::new();
    let mut cursor = 0_u64;
    for (entry_index, entry) in entries.iter().enumerate() {
        let time = number_attr::<u64>(entry, "t")?.unwrap_or(cursor);
        if !spans.is_empty() && time < cursor {
            return Err(invalid_segments("SegmentTimeline entries overlap"));
        }
        let duration = number_attr::<u64>(entry, "d")?
            .filter(|duration| *duration > 0)
            .ok_or_else(|| invalid_segments("timeline S requires positive d"))?;
        let repeat = entry
            .attr("r")
            .map(|value| {
                value
                    .parse::<i64>()
                    .map_err(|_| DashParseError::InvalidNumber {
                        attribute: "r".to_owned(),
                        value: value.to_owned(),
                    })
            })
            .transpose()?
            .unwrap_or(0);
        let count = if repeat >= 0 {
            u64::try_from(repeat)
                .ok()
                .and_then(|repeat| repeat.checked_add(1))
                .ok_or_else(|| invalid_segments("timeline repeat overflow"))?
        } else if repeat == -1 {
            let boundary = entries
                .get(entry_index + 1)
                .and_then(|next| next.attr("t"))
                .map(|value| {
                    value
                        .parse::<u64>()
                        .map_err(|_| DashParseError::InvalidNumber {
                            attribute: "t".to_owned(),
                            value: value.to_owned(),
                        })
                })
                .transpose()?
                .or(period_end)
                .ok_or_else(|| {
                    invalid_segments("r=-1 requires a following t or Period duration")
                })?;
            if boundary <= time {
                return Err(invalid_segments("r=-1 has a non-increasing boundary"));
            }
            boundary.saturating_sub(time).div_ceil(duration)
        } else {
            return Err(invalid_segments("timeline r must be -1 or non-negative"));
        };
        check_segment_count((spans.len() as u64).saturating_add(count))?;
        for offset in 0..count {
            let segment_time = offset
                .checked_mul(duration)
                .and_then(|offset| time.checked_add(offset))
                .ok_or_else(|| invalid_segments("timeline time overflow"))?;
            spans.push((segment_time, duration));
        }
        cursor = time
            .checked_add(
                count
                    .checked_mul(duration)
                    .ok_or_else(|| invalid_segments("timeline time overflow"))?,
            )
            .ok_or_else(|| invalid_segments("timeline time overflow"))?;
    }
    Ok(spans)
}

fn expand_segment_list(
    scopes: &[&XmlNode; 3],
    base_url: &Url,
    period_duration_millis: Option<u64>,
) -> Result<DashSegmentPlan, DashParseError> {
    let lists = scopes
        .iter()
        .filter_map(|scope| scope.children_named("SegmentList").next())
        .collect::<Vec<_>>();
    let timescale = inherited_number_attr(&lists, "timescale")?.unwrap_or(1);
    if timescale == 0 {
        return Err(invalid_segments("SegmentList timescale must be positive"));
    }
    let duration = inherited_number_attr::<u64>(&lists, "duration")?;
    let start_number = inherited_number_attr::<u64>(&lists, "startNumber")?.unwrap_or(1);
    let initialization_node = lists
        .iter()
        .rev()
        .find_map(|list| list.children_named("Initialization").next());
    let initialization = initialization_node
        .map(|node| {
            let source = node.attr("sourceURL").unwrap_or("");
            Ok(DashSegmentResource {
                url: resolve_segment_url(base_url, source)?,
                byte_range: node.attr("range").map(parse_byte_range).transpose()?,
            })
        })
        .transpose()?;
    let segment_urls = lists
        .iter()
        .rev()
        .find_map(|list| {
            let urls = list.children_named("SegmentURL").collect::<Vec<_>>();
            (!urls.is_empty()).then_some(urls)
        })
        .ok_or_else(|| invalid_segments("SegmentList requires SegmentURL entries"))?;
    check_segment_count(segment_urls.len() as u64)?;
    let timeline = lists
        .iter()
        .rev()
        .find_map(|list| list.children_named("SegmentTimeline").next());
    let spans = if let Some(timeline) = timeline {
        let spans = expand_timeline(timeline, period_duration_millis, timescale, 0)?;
        if spans.len() != segment_urls.len() {
            return Err(invalid_segments(
                "SegmentTimeline and SegmentURL counts differ",
            ));
        }
        spans
    } else {
        let duration = duration.filter(|duration| *duration > 0).ok_or_else(|| {
            invalid_segments("SegmentList requires positive duration or timeline")
        })?;
        (0..segment_urls.len())
            .map(|index| {
                (index as u64)
                    .checked_mul(duration)
                    .map(|time| (time, duration))
                    .ok_or_else(|| invalid_segments("segment time overflow"))
            })
            .collect::<Result<Vec<_>, _>>()?
    };
    let media = segment_urls
        .into_iter()
        .zip(spans)
        .enumerate()
        .map(|(index, (node, (time, duration)))| {
            let media = node
                .attr("media")
                .ok_or_else(|| invalid_segments("SegmentURL requires media"))?;
            Ok(DashMediaSegment {
                number: start_number
                    .checked_add(index as u64)
                    .ok_or_else(|| invalid_segments("segment number overflow"))?,
                time,
                duration,
                resource: DashSegmentResource {
                    url: resolve_segment_url(base_url, media)?,
                    byte_range: node.attr("mediaRange").map(parse_byte_range).transpose()?,
                },
            })
        })
        .collect::<Result<Vec<_>, DashParseError>>()?;
    Ok(DashSegmentPlan {
        initialization,
        media,
    })
}

fn inherited_number_attr<T>(nodes: &[&XmlNode], name: &str) -> Result<Option<T>, DashParseError>
where
    T: std::str::FromStr,
{
    nodes
        .iter()
        .rev()
        .find_map(|node| node.attr(name))
        .map(|value| {
            value.parse().map_err(|_| DashParseError::InvalidNumber {
                attribute: name.to_owned(),
                value: value.to_owned(),
            })
        })
        .transpose()
}

fn substitute_template(
    template: &str,
    representation_id: Option<&str>,
    bandwidth: Option<u64>,
    number: u64,
    time: u64,
) -> Result<String, DashParseError> {
    let mut output = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(open) = rest.find('$') {
        output.push_str(&rest[..open]);
        rest = &rest[open + 1..];
        if let Some(escaped) = rest.strip_prefix('$') {
            output.push('$');
            rest = escaped;
            continue;
        }
        let close = rest
            .find('$')
            .ok_or_else(|| invalid_segments("unterminated SegmentTemplate identifier"))?;
        let token = &rest[..close];
        rest = &rest[close + 1..];
        let (name, width) = parse_template_token(token)?;
        match name {
            "RepresentationID" => output.push_str(representation_id.ok_or_else(|| {
                invalid_segments("$RepresentationID$ requires a representation id")
            })?),
            "Bandwidth" => push_formatted_number(
                &mut output,
                bandwidth.ok_or_else(|| {
                    invalid_segments("$Bandwidth$ requires representation bandwidth")
                })?,
                width,
            ),
            "Number" => push_formatted_number(&mut output, number, width),
            "Time" => push_formatted_number(&mut output, time, width),
            _ => return Err(invalid_segments("unsupported SegmentTemplate identifier")),
        }
    }
    output.push_str(rest);
    Ok(output)
}

fn parse_template_token(token: &str) -> Result<(&str, usize), DashParseError> {
    let Some(percent) = token.find('%') else {
        return Ok((token, 0));
    };
    let name = &token[..percent];
    let format = &token[percent + 1..];
    if !format.starts_with('0') || !format.ends_with('d') {
        return Err(invalid_segments("invalid SegmentTemplate number format"));
    }
    let width = format[1..format.len() - 1]
        .parse::<usize>()
        .map_err(|_| invalid_segments("invalid SegmentTemplate number width"))?;
    if width > 20 {
        return Err(invalid_segments("SegmentTemplate number width exceeds 20"));
    }
    Ok((name, width))
}

fn push_formatted_number(output: &mut String, value: u64, width: usize) {
    use std::fmt::Write as _;
    let _ = write!(output, "{value:0width$}");
}

fn parse_byte_range(value: &str) -> Result<DashByteRange, DashParseError> {
    let (start, end) = value
        .split_once('-')
        .ok_or_else(|| invalid_segments("byte range must be start-end"))?;
    let start = start
        .parse::<u64>()
        .map_err(|_| invalid_segments("invalid byte range start"))?;
    let end = end
        .parse::<u64>()
        .map_err(|_| invalid_segments("invalid byte range end"))?;
    if start > end {
        return Err(invalid_segments("byte range start exceeds end"));
    }
    Ok(DashByteRange { start, end })
}

fn resolve_segment_url(base_url: &Url, value: &str) -> Result<Url, DashParseError> {
    base_url
        .join(value)
        .map_err(|_| invalid_segments("invalid segment URL"))
}

fn period_ticks(
    duration_millis: Option<u64>,
    timescale: u64,
) -> Result<Option<u64>, DashParseError> {
    duration_millis
        .map(|duration| {
            duration
                .checked_mul(timescale)
                .and_then(|scaled| scaled.checked_add(999))
                .map(|scaled| scaled / 1_000)
                .ok_or_else(|| invalid_segments("Period duration overflows timescale"))
        })
        .transpose()
}

fn duration_attr_millis(node: &XmlNode, attribute: &str) -> Result<Option<u64>, DashParseError> {
    node.attr(attribute)
        .map(|value| {
            parse_iso_duration_millis(value).ok_or_else(|| DashParseError::InvalidDuration {
                attribute: attribute.to_owned(),
                value: value.to_owned(),
            })
        })
        .transpose()
}

fn parse_iso_duration_millis(value: &str) -> Option<u64> {
    let mut rest = value.strip_prefix('P')?;
    if rest.is_empty() {
        return None;
    }
    let mut total = 0_u64;
    let mut in_time = false;
    let mut saw_value = false;
    while !rest.is_empty() {
        if let Some(after_t) = rest.strip_prefix('T') {
            if in_time {
                return None;
            }
            in_time = true;
            rest = after_t;
            continue;
        }
        let end = rest.find(|character: char| !character.is_ascii_digit() && character != '.')?;
        let number = &rest[..end];
        let unit = rest.as_bytes().get(end).copied()? as char;
        rest = &rest[end + 1..];
        let unit_millis = match (in_time, unit) {
            (false, 'D') => 86_400_000,
            (true, 'H') => 3_600_000,
            (true, 'M') => 60_000,
            (true, 'S') => 1_000,
            _ => return None,
        };
        let millis = if unit == 'S' && number.contains('.') {
            let (whole, fraction) = number.split_once('.')?;
            if fraction.is_empty()
                || fraction.len() > 3
                || !fraction.bytes().all(|b| b.is_ascii_digit())
            {
                return None;
            }
            let whole = whole.parse::<u64>().ok()?.checked_mul(1_000)?;
            let scale = match fraction.len() {
                1 => 100,
                2 => 10,
                3 => 1,
                _ => return None,
            };
            let fraction = fraction.parse::<u64>().ok()? * scale;
            whole.checked_add(fraction)?
        } else {
            number.parse::<u64>().ok()?.checked_mul(unit_millis)?
        };
        total = total.checked_add(millis)?;
        saw_value = true;
    }
    saw_value.then_some(total)
}

fn check_segment_count(count: u64) -> Result<(), DashParseError> {
    if count > MAX_EXPANDED_SEGMENTS as u64 {
        Err(DashParseError::SegmentLimit)
    } else {
        Ok(())
    }
}

fn invalid_segments(message: &str) -> DashParseError {
    DashParseError::InvalidSegments(message.to_owned())
}

fn parse_content_protections(node: &XmlNode) -> Vec<DashContentProtection> {
    node.children_named("ContentProtection")
        .map(|protection| DashContentProtection {
            scheme_id_uri: owned_attr(protection, "schemeIdUri"),
            value: owned_attr(protection, "value"),
            default_kid: owned_attr(protection, "default_KID"),
            pssh: protection
                .children_named("pssh")
                .next()
                .map(|node| node.text.trim().to_owned())
                .filter(|value| !value.is_empty()),
        })
        .collect()
}

fn resolve_base_url(node: &XmlNode, parent: &Url) -> Result<Url, DashParseError> {
    let Some(value) = node
        .children_named("BaseURL")
        .map(|base| base.text.trim())
        .find(|value| !value.is_empty())
    else {
        return Ok(parent.clone());
    };
    parent
        .join(value)
        .map_err(|_| DashParseError::InvalidBaseUrl {
            value: value.to_owned(),
        })
}

fn number_attr<T>(node: &XmlNode, name: &str) -> Result<Option<T>, DashParseError>
where
    T: std::str::FromStr,
{
    node.attr(name)
        .map(|value| {
            value.parse().map_err(|_| DashParseError::InvalidNumber {
                attribute: name.to_owned(),
                value: value.to_owned(),
            })
        })
        .transpose()
}

fn owned_attr(node: &XmlNode, name: &str) -> Option<String> {
    node.attr(name).map(str::to_owned)
}

#[derive(Debug)]
struct XmlNode {
    name: String,
    attributes: Vec<(String, String)>,
    text: String,
    children: Vec<Self>,
}

impl XmlNode {
    fn attr(&self, name: &str) -> Option<&str> {
        self.attributes
            .iter()
            .find(|(candidate, _)| candidate == name)
            .map(|(_, value)| value.as_str())
    }

    fn children_named<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a Self> {
        self.children.iter().filter(move |node| node.name == name)
    }
}

fn parse_xml(document: &str) -> Result<XmlNode, DashParseError> {
    let mut reader = Reader::from_str(document);
    reader.config_mut().trim_text(true);
    let mut stack = Vec::<XmlNode>::new();
    let mut root = None;
    let mut node_count = 0_usize;

    loop {
        match reader.read_event() {
            Ok(Event::Start(element)) => {
                node_count += 1;
                if node_count > MAX_XML_NODES || stack.len() >= MAX_XML_DEPTH {
                    return Err(DashParseError::ComplexityLimit);
                }
                stack.push(xml_node(&reader, &element)?);
            }
            Ok(Event::Empty(element)) => {
                node_count += 1;
                if node_count > MAX_XML_NODES || stack.len() >= MAX_XML_DEPTH {
                    return Err(DashParseError::ComplexityLimit);
                }
                append_node(xml_node(&reader, &element)?, &mut stack, &mut root)?;
            }
            Ok(Event::Text(text)) => {
                if let Some(node) = stack.last_mut() {
                    let decoded = text
                        .xml_content()
                        .map_err(|error| DashParseError::MalformedXml(error.to_string()))?;
                    node.text.push_str(&decoded);
                }
            }
            Ok(Event::CData(text)) => {
                if let Some(node) = stack.last_mut() {
                    let decoded = text
                        .decode()
                        .map_err(|error| DashParseError::MalformedXml(error.to_string()))?;
                    node.text.push_str(&decoded);
                }
            }
            Ok(Event::End(_)) => {
                let node = stack.pop().ok_or_else(|| {
                    DashParseError::MalformedXml("unexpected closing element".to_owned())
                })?;
                append_node(node, &mut stack, &mut root)?;
            }
            Ok(Event::Eof) => break,
            Ok(Event::DocType(_)) => {
                return Err(DashParseError::MalformedXml(
                    "document types are not accepted".to_owned(),
                ));
            }
            Ok(Event::GeneralRef(reference)) => {
                if let Some(node) = stack.last_mut() {
                    if let Some(character) = reference
                        .resolve_char_ref()
                        .map_err(|error| DashParseError::MalformedXml(error.to_string()))?
                    {
                        node.text.push(character);
                    } else {
                        let name = reference
                            .decode()
                            .map_err(|error| DashParseError::MalformedXml(error.to_string()))?;
                        node.text.push(match name.as_ref() {
                            "lt" => '<',
                            "gt" => '>',
                            "amp" => '&',
                            "apos" => '\'',
                            "quot" => '"',
                            _ => {
                                return Err(DashParseError::MalformedXml(format!(
                                    "unknown entity reference &{name};"
                                )));
                            }
                        });
                    }
                }
            }
            Ok(Event::Decl(_) | Event::Comment(_) | Event::PI(_)) => {}
            Err(error) => return Err(DashParseError::MalformedXml(error.to_string())),
        }
    }
    if !stack.is_empty() {
        return Err(DashParseError::MalformedXml(
            "unclosed XML element".to_owned(),
        ));
    }
    root.ok_or(DashParseError::MissingMpd)
}

fn xml_node(
    reader: &Reader<&[u8]>,
    element: &quick_xml::events::BytesStart<'_>,
) -> Result<XmlNode, DashParseError> {
    let name = local_name(element.name().as_ref())?;
    let mut attributes = Vec::new();
    for attribute in element.attributes() {
        let attribute =
            attribute.map_err(|error| DashParseError::MalformedXml(error.to_string()))?;
        let key = local_name(attribute.key.as_ref())?;
        let value = attribute
            .decode_and_unescape_value(reader.decoder())
            .map_err(|error| DashParseError::MalformedXml(error.to_string()))?
            .into_owned();
        attributes.push((key, value));
    }
    Ok(XmlNode {
        name,
        attributes,
        text: String::new(),
        children: Vec::new(),
    })
}

fn local_name(name: &[u8]) -> Result<String, DashParseError> {
    let local = name.rsplit(|byte| *byte == b':').next().unwrap_or(name);
    std::str::from_utf8(local)
        .map(str::to_owned)
        .map_err(|error| DashParseError::MalformedXml(error.to_string()))
}

fn append_node(
    node: XmlNode,
    stack: &mut [XmlNode],
    root: &mut Option<XmlNode>,
) -> Result<(), DashParseError> {
    if let Some(parent) = stack.last_mut() {
        parent.children.push(node);
        return Ok(());
    }
    if root.replace(node).is_some() {
        return Err(DashParseError::MalformedXml(
            "multiple root elements".to_owned(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{DashParseError, MAX_MPD_BYTES, parse_static_mpd};
    use url::Url;

    fn manifest_url() -> Url {
        Url::parse("https://cdn.example.test/root/manifest.mpd").expect("manifest URL")
    }

    #[test]
    fn parses_multi_period_adaptations_and_nested_base_urls() {
        let document = r#"<?xml version="1.0"?>
            <MPD type="static">
              <BaseURL>media/</BaseURL>
              <Period id="intro">
                <BaseURL>p0/</BaseURL>
                <AdaptationSet id="video" contentType="video" mimeType="video/mp4" codecs="avc1.640028">
                  <BaseURL>video/</BaseURL>
                  <Representation id="1080" bandwidth="4500000" width="1920" height="1080" frameRate="30000/1001">
                    <BaseURL>high/</BaseURL>
                  </Representation>
                  <Representation id="720" bandwidth="2200000" width="1280" height="720" codecs="avc1.4d401f" />
                </AdaptationSet>
              </Period>
              <Period id="main">
                <AdaptationSet id="audio" contentType="audio" mimeType="audio/mp4" lang="bn">
                  <Representation id="bn-128" bandwidth="128000" codecs="mp4a.40.2" />
                </AdaptationSet>
              </Period>
            </MPD>"#;

        let manifest = parse_static_mpd(document, &manifest_url()).expect("static MPD");
        assert_eq!(
            manifest.base_url.as_str(),
            "https://cdn.example.test/root/media/"
        );
        assert_eq!(manifest.periods.len(), 2);
        let video = &manifest.periods[0].adaptations[0];
        assert_eq!(video.representations.len(), 2);
        assert_eq!(
            video.representations[0].base_url.as_str(),
            "https://cdn.example.test/root/media/p0/video/high/"
        );
        assert_eq!(
            video.representations[0].mime_type.as_deref(),
            Some("video/mp4")
        );
        assert_eq!(
            video.representations[0].codecs.as_deref(),
            Some("avc1.640028")
        );
        assert_eq!(
            video.representations[1].codecs.as_deref(),
            Some("avc1.4d401f")
        );
        assert_eq!(
            manifest.periods[1].adaptations[0].language.as_deref(),
            Some("bn")
        );
    }

    #[test]
    fn preserves_namespaced_drm_markers_without_interpreting_keys() {
        let document = r#"<MPD xmlns:cenc="urn:mpeg:cenc:2013" type="static">
          <Period><AdaptationSet contentType="video">
            <ContentProtection schemeIdUri="urn:mpeg:dash:mp4protection:2011" value="cenc" />
            <ContentProtection schemeIdUri="urn:uuid:edef8ba9-79d6-4ace-a3c8-27dcd51d21ed" cenc:default_KID="00112233-4455-6677-8899-aabbccddeeff">
              <cenc:pssh>AAAAV3Bzc2g=</cenc:pssh>
            </ContentProtection>
            <Representation id="protected" bandwidth="1000">
              <ContentProtection schemeIdUri="urn:uuid:9a04f079-9840-4286-ab92-e65be0885f95" />
            </Representation>
          </AdaptationSet></Period>
        </MPD>"#;

        let manifest = parse_static_mpd(document, &manifest_url()).expect("protected MPD");
        let adaptation = &manifest.periods[0].adaptations[0];
        assert_eq!(adaptation.content_protections.len(), 2);
        assert_eq!(
            adaptation.content_protections[0].value.as_deref(),
            Some("cenc")
        );
        assert_eq!(
            adaptation.content_protections[1].default_kid.as_deref(),
            Some("00112233-4455-6677-8899-aabbccddeeff")
        );
        assert_eq!(
            adaptation.content_protections[1].pssh.as_deref(),
            Some("AAAAV3Bzc2g=")
        );
        assert_eq!(
            adaptation.representations[0].content_protections[0]
                .scheme_id_uri
                .as_deref(),
            Some("urn:uuid:9a04f079-9840-4286-ab92-e65be0885f95")
        );
    }

    #[test]
    fn expands_inherited_duration_template_with_safe_substitution() {
        let document = r#"<MPD type="static" mediaPresentationDuration="PT5.5S">
          <Period><AdaptationSet contentType="video">
            <SegmentTemplate timescale="1000" duration="2000" startNumber="7"
              initialization="init-$RepresentationID$-$Bandwidth$.mp4"
              media="chunk-$Number%03d$-$Time$-$$.m4s" />
            <Representation id="main" bandwidth="800000" />
          </AdaptationSet></Period>
        </MPD>"#;

        let manifest = parse_static_mpd(document, &manifest_url()).expect("template MPD");
        assert_eq!(manifest.periods[0].duration_millis, Some(5_500));
        let plan = manifest.periods[0].adaptations[0].representations[0]
            .segment_plan
            .as_ref()
            .expect("expanded plan");
        assert_eq!(
            plan.initialization.as_ref().expect("init").url.as_str(),
            "https://cdn.example.test/root/init-main-800000.mp4"
        );
        assert_eq!(plan.media.len(), 3);
        assert_eq!(
            plan.media
                .iter()
                .map(|segment| (segment.number, segment.time, segment.duration))
                .collect::<Vec<_>>(),
            vec![(7, 0, 2_000), (8, 2_000, 2_000), (9, 4_000, 2_000)]
        );
        assert_eq!(
            plan.media[2].resource.url.as_str(),
            "https://cdn.example.test/root/chunk-009-4000-$.m4s"
        );
    }

    #[test]
    fn expands_timeline_repeats_to_next_start_and_period_boundary() {
        let document = r#"<MPD type="static"><Period duration="PT25S">
          <AdaptationSet><Representation id="v">
            <SegmentTemplate timescale="1" media="$Time$.m4s">
              <SegmentTimeline>
                <S t="0" d="5" r="-1" />
                <S t="15" d="5" r="-1" />
              </SegmentTimeline>
            </SegmentTemplate>
          </Representation></AdaptationSet>
        </Period></MPD>"#;

        let manifest = parse_static_mpd(document, &manifest_url()).expect("timeline MPD");
        let media = &manifest.periods[0].adaptations[0].representations[0]
            .segment_plan
            .as_ref()
            .expect("expanded plan")
            .media;
        assert_eq!(
            media.iter().map(|segment| segment.time).collect::<Vec<_>>(),
            vec![0, 5, 10, 15, 20]
        );
        assert_eq!(media[4].resource.url.path(), "/root/20.m4s");
    }

    #[test]
    fn expands_segment_list_urls_timing_and_inclusive_ranges() {
        let document = r#"<MPD type="static"><Period><AdaptationSet>
          <SegmentList timescale="10" duration="25" startNumber="4">
            <Initialization sourceURL="media.mp4" range="0-99" />
            <SegmentURL media="media.mp4" mediaRange="100-199" />
            <SegmentURL media="tail.m4s" />
          </SegmentList>
          <Representation id="audio" />
        </AdaptationSet></Period></MPD>"#;

        let manifest = parse_static_mpd(document, &manifest_url()).expect("list MPD");
        let plan = manifest.periods[0].adaptations[0].representations[0]
            .segment_plan
            .as_ref()
            .expect("expanded plan");
        assert_eq!(
            plan.initialization.as_ref().expect("init").byte_range,
            Some(super::DashByteRange { start: 0, end: 99 })
        );
        assert_eq!(plan.media[0].number, 4);
        assert_eq!(plan.media[1].time, 25);
        assert_eq!(
            plan.media[0].resource.byte_range,
            Some(super::DashByteRange {
                start: 100,
                end: 199
            })
        );
        assert_eq!(plan.media[1].resource.url.path(), "/root/tail.m4s");
    }

    #[test]
    fn rejects_unbounded_and_malformed_segment_expansion() {
        let invalid = [
            r#"<MPD type="static"><Period><AdaptationSet><Representation><SegmentTemplate media="$Number$.m4s"><SegmentTimeline><S d="2" r="-1" /></SegmentTimeline></SegmentTemplate></Representation></AdaptationSet></Period></MPD>"#,
            r#"<MPD type="static"><Period duration="PT10S"><AdaptationSet><Representation><SegmentTemplate media="$Number$.m4s"><SegmentTimeline><S d="2" r="-2" /></SegmentTimeline></SegmentTemplate></Representation></AdaptationSet></Period></MPD>"#,
            r#"<MPD type="static"><Period><AdaptationSet><Representation><SegmentList duration="1"><SegmentURL media="x" mediaRange="9-1" /></SegmentList></Representation></AdaptationSet></Period></MPD>"#,
            r#"<MPD type="static"><Period><AdaptationSet><Representation><SegmentList><SegmentTimeline><S d="1" r="1" /></SegmentTimeline><SegmentURL media="one" /></SegmentList></Representation></AdaptationSet></Period></MPD>"#,
        ];
        for document in invalid {
            assert!(matches!(
                parse_static_mpd(document, &manifest_url()),
                Err(DashParseError::InvalidSegments(_))
            ));
        }
        let excessive = r#"<MPD type="static" mediaPresentationDuration="PT100001S"><Period><AdaptationSet><Representation><SegmentTemplate duration="1" media="$Number$.m4s" /></Representation></AdaptationSet></Period></MPD>"#;
        assert_eq!(
            parse_static_mpd(excessive, &manifest_url()),
            Err(DashParseError::SegmentLimit)
        );
    }

    #[test]
    fn rejects_dynamic_malformed_and_invalid_static_metadata() {
        assert_eq!(
            parse_static_mpd("<MPD type='dynamic'/>", &manifest_url()),
            Err(DashParseError::DynamicManifest)
        );
        assert!(matches!(
            parse_static_mpd("<MPD><Period></MPD>", &manifest_url()),
            Err(DashParseError::MalformedXml(_))
        ));
        assert!(
            matches!(parse_static_mpd("<MPD><Period><AdaptationSet><Representation bandwidth='fast'/></AdaptationSet></Period></MPD>", &manifest_url()), Err(DashParseError::InvalidNumber { attribute, .. }) if attribute == "bandwidth")
        );
        assert!(matches!(
            parse_static_mpd(
                "<MPD><BaseURL>http://[invalid</BaseURL></MPD>",
                &manifest_url()
            ),
            Err(DashParseError::InvalidBaseUrl { .. })
        ));
        let oversized = format!("<MPD>{}</MPD>", " ".repeat(MAX_MPD_BYTES));
        assert_eq!(
            parse_static_mpd(&oversized, &manifest_url()),
            Err(DashParseError::DocumentTooLarge)
        );
        let too_deep = format!(
            "<MPD>{}{}</MPD>",
            "<Nested>".repeat(32),
            "</Nested>".repeat(32)
        );
        assert_eq!(
            parse_static_mpd(&too_deep, &manifest_url()),
            Err(DashParseError::ComplexityLimit)
        );
    }
}
