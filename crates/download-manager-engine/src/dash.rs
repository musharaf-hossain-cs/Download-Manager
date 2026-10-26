//! Bounded parsing of static MPEG-DASH adaptation and representation metadata.

use quick_xml::{Reader, events::Event};
use thiserror::Error;
use url::Url;

const MAX_MPD_BYTES: usize = 1_048_576;
const MAX_XML_DEPTH: usize = 32;
const MAX_XML_NODES: usize = 20_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DashManifest {
    pub base_url: Url,
    pub periods: Vec<DashPeriod>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DashPeriod {
    pub id: Option<String>,
    pub base_url: Url,
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
}

/// Parses adaptation sets and representations from a bounded static DASH MPD.
///
/// Relative `BaseURL` values are resolved at each MPD, Period, `AdaptationSet`, and
/// Representation level. `SegmentTemplate`, `SegmentTimeline`, and `SegmentList` are
/// intentionally left to the S111 expansion contract.
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
    let periods = root
        .children_named("Period")
        .map(|node| parse_period(node, &base_url))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(DashManifest { base_url, periods })
}

fn parse_period(node: &XmlNode, parent_url: &Url) -> Result<DashPeriod, DashParseError> {
    let base_url = resolve_base_url(node, parent_url)?;
    let adaptations = node
        .children_named("AdaptationSet")
        .map(|adaptation| parse_adaptation(adaptation, &base_url))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(DashPeriod {
        id: owned_attr(node, "id"),
        base_url,
        adaptations,
    })
}

fn parse_adaptation(node: &XmlNode, parent_url: &Url) -> Result<DashAdaptation, DashParseError> {
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
) -> Result<DashRepresentation, DashParseError> {
    Ok(DashRepresentation {
        id: owned_attr(node, "id"),
        bandwidth: number_attr(node, "bandwidth")?,
        width: number_attr(node, "width")?,
        height: number_attr(node, "height")?,
        frame_rate: owned_attr(node, "frameRate"),
        mime_type: owned_attr(node, "mimeType").or_else(|| inherited_mime_type.map(str::to_owned)),
        codecs: owned_attr(node, "codecs").or_else(|| inherited_codecs.map(str::to_owned)),
        base_url: resolve_base_url(node, parent_url)?,
        content_protections: parse_content_protections(node),
    })
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
