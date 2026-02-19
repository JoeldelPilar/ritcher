use crate::error::{Result, RitcherError};
use quick_xml::events::Event;
use quick_xml::reader::Reader;
use tracing::{info, warn};

/// Parsed VAST response containing ads
#[derive(Debug, Clone)]
pub struct VastResponse {
    pub version: String,
    pub ads: Vec<VastAd>,
}

/// A single ad from a VAST response
#[derive(Debug, Clone)]
pub struct VastAd {
    pub id: String,
    pub ad_type: VastAdType,
}

/// InLine ad (actual creative) or Wrapper (redirect to another VAST)
#[derive(Debug, Clone)]
pub enum VastAdType {
    InLine(InLineAd),
    Wrapper(WrapperAd),
}

/// InLine ad with creative content
#[derive(Debug, Clone)]
pub struct InLineAd {
    pub ad_system: String,
    pub ad_title: String,
    pub creatives: Vec<Creative>,
    pub impression_urls: Vec<String>,
    pub error_url: Option<String>,
}

/// Wrapper ad that references another VAST tag
#[derive(Debug, Clone)]
pub struct WrapperAd {
    pub ad_tag_uri: String,
    pub impression_urls: Vec<String>,
    pub tracking_events: Vec<TrackingEvent>,
}

/// A creative containing linear video content
#[derive(Debug, Clone)]
pub struct Creative {
    pub id: String,
    pub linear: Option<LinearAd>,
}

/// Linear (video) ad content
#[derive(Debug, Clone)]
pub struct LinearAd {
    pub duration: f32,
    pub media_files: Vec<MediaFile>,
    pub tracking_events: Vec<TrackingEvent>,
}

/// A single media file for an ad creative
#[derive(Debug, Clone)]
pub struct MediaFile {
    pub url: String,
    pub delivery: String,
    pub mime_type: String,
    pub width: u32,
    pub height: u32,
    pub bitrate: Option<u32>,
    pub codec: Option<String>,
}

/// Tracking event for ad playback reporting
#[derive(Debug, Clone, PartialEq)]
pub struct TrackingEvent {
    pub event: String,
    pub url: String,
}

/// Parse VAST XML into structured data
pub fn parse_vast(xml: &str) -> Result<VastResponse> {
    let mut reader = Reader::from_str(xml);

    let mut version = String::new();
    let mut ads = Vec::new();

    loop {
        match reader.read_event() {
            Ok(Event::Start(ref e)) if e.name().as_ref() == b"VAST" => {
                version = get_attr(e, "version").unwrap_or_default();
                info!("Parsing VAST version {}", version);
            }
            Ok(Event::Start(ref e)) if e.name().as_ref() == b"Ad" => {
                let ad_id = get_attr(e, "id").unwrap_or_default();
                if let Some(ad) = parse_ad(&mut reader, ad_id)? {
                    ads.push(ad);
                }
            }
            Ok(Event::Eof) => break,
            Err(e) => {
                return Err(RitcherError::InternalError(format!(
                    "VAST XML parse error: {}",
                    e
                )));
            }
            _ => {}
        }
    }

    if ads.is_empty() {
        info!("VAST response contains no ads (empty response)");
    } else {
        info!("Parsed {} ad(s) from VAST response", ads.len());
    }

    Ok(VastResponse { version, ads })
}

/// Parse a single <Ad> element
fn parse_ad(reader: &mut Reader<&[u8]>, id: String) -> Result<Option<VastAd>> {
    loop {
        match reader.read_event() {
            Ok(Event::Start(ref e)) if e.name().as_ref() == b"InLine" => {
                let inline = parse_inline(reader)?;
                return Ok(Some(VastAd {
                    id,
                    ad_type: VastAdType::InLine(inline),
                }));
            }
            Ok(Event::Start(ref e)) if e.name().as_ref() == b"Wrapper" => {
                let wrapper = parse_wrapper(reader)?;
                return Ok(Some(VastAd {
                    id,
                    ad_type: VastAdType::Wrapper(wrapper),
                }));
            }
            Ok(Event::End(ref e)) if e.name().as_ref() == b"Ad" => return Ok(None),
            Ok(Event::Eof) => return Ok(None),
            Err(e) => {
                return Err(RitcherError::InternalError(format!(
                    "VAST XML parse error in Ad: {}",
                    e
                )));
            }
            _ => {}
        }
    }
}

/// Parse <InLine> element
fn parse_inline(reader: &mut Reader<&[u8]>) -> Result<InLineAd> {
    let mut ad_system = String::new();
    let mut ad_title = String::new();
    let mut creatives = Vec::new();
    let mut impression_urls = Vec::new();
    let mut error_url = None;

    loop {
        match reader.read_event() {
            Ok(Event::Start(ref e)) if e.name().as_ref() == b"AdSystem" => {
                ad_system = read_text(reader, "AdSystem")?;
            }
            Ok(Event::Start(ref e)) if e.name().as_ref() == b"AdTitle" => {
                ad_title = read_text(reader, "AdTitle")?;
            }
            Ok(Event::Start(ref e)) if e.name().as_ref() == b"Impression" => {
                let url = read_text(reader, "Impression")?;
                if !url.is_empty() {
                    impression_urls.push(url);
                }
            }
            Ok(Event::Start(ref e)) if e.name().as_ref() == b"Error" => {
                error_url = Some(read_text(reader, "Error")?);
            }
            Ok(Event::Start(ref e)) if e.name().as_ref() == b"Creatives" => {
                creatives = parse_creatives(reader)?;
            }
            Ok(Event::End(ref e)) if e.name().as_ref() == b"InLine" => break,
            Ok(Event::Eof) => break,
            Err(e) => {
                return Err(RitcherError::InternalError(format!(
                    "VAST XML parse error in InLine: {}",
                    e
                )));
            }
            _ => {}
        }
    }

    Ok(InLineAd {
        ad_system,
        ad_title,
        creatives,
        impression_urls,
        error_url,
    })
}

/// Parse <Wrapper> element
fn parse_wrapper(reader: &mut Reader<&[u8]>) -> Result<WrapperAd> {
    let mut ad_tag_uri = String::new();
    let mut impression_urls = Vec::new();
    let mut tracking_events = Vec::new();

    loop {
        match reader.read_event() {
            Ok(Event::Start(ref e)) if e.name().as_ref() == b"VASTAdTagURI" => {
                ad_tag_uri = read_text(reader, "VASTAdTagURI")?;
            }
            Ok(Event::Start(ref e)) if e.name().as_ref() == b"Impression" => {
                let url = read_text(reader, "Impression")?;
                if !url.is_empty() {
                    impression_urls.push(url);
                }
            }
            Ok(Event::Start(ref e)) if e.name().as_ref() == b"TrackingEvents" => {
                tracking_events = parse_tracking_events(reader)?;
            }
            Ok(Event::End(ref e)) if e.name().as_ref() == b"Wrapper" => break,
            Ok(Event::Eof) => break,
            Err(e) => {
                return Err(RitcherError::InternalError(format!(
                    "VAST XML parse error in Wrapper: {}",
                    e
                )));
            }
            _ => {}
        }
    }

    Ok(WrapperAd {
        ad_tag_uri,
        impression_urls,
        tracking_events,
    })
}

/// Parse <Creatives> element
fn parse_creatives(reader: &mut Reader<&[u8]>) -> Result<Vec<Creative>> {
    let mut creatives = Vec::new();

    loop {
        match reader.read_event() {
            Ok(Event::Start(ref e)) if e.name().as_ref() == b"Creative" => {
                let id = get_attr(e, "id").unwrap_or_default();
                let creative = parse_creative(reader, id)?;
                creatives.push(creative);
            }
            Ok(Event::End(ref e)) if e.name().as_ref() == b"Creatives" => break,
            Ok(Event::Eof) => break,
            Err(e) => {
                return Err(RitcherError::InternalError(format!(
                    "VAST XML parse error in Creatives: {}",
                    e
                )));
            }
            _ => {}
        }
    }

    Ok(creatives)
}

/// Parse a single <Creative> element
fn parse_creative(reader: &mut Reader<&[u8]>, id: String) -> Result<Creative> {
    let mut linear = None;

    loop {
        match reader.read_event() {
            Ok(Event::Start(ref e)) if e.name().as_ref() == b"Linear" => {
                linear = Some(parse_linear(reader)?);
            }
            Ok(Event::End(ref e)) if e.name().as_ref() == b"Creative" => break,
            Ok(Event::Eof) => break,
            Err(e) => {
                return Err(RitcherError::InternalError(format!(
                    "VAST XML parse error in Creative: {}",
                    e
                )));
            }
            _ => {}
        }
    }

    Ok(Creative { id, linear })
}

/// Parse <Linear> element
fn parse_linear(reader: &mut Reader<&[u8]>) -> Result<LinearAd> {
    let mut duration = 0.0;
    let mut media_files = Vec::new();
    let mut tracking_events = Vec::new();

    loop {
        match reader.read_event() {
            Ok(Event::Start(ref e)) if e.name().as_ref() == b"Duration" => {
                let dur_str = read_text(reader, "Duration")?;
                duration = parse_duration(&dur_str);
            }
            Ok(Event::Start(ref e)) if e.name().as_ref() == b"MediaFiles" => {
                media_files = parse_media_files(reader)?;
            }
            Ok(Event::Start(ref e)) if e.name().as_ref() == b"TrackingEvents" => {
                tracking_events = parse_tracking_events(reader)?;
            }
            Ok(Event::End(ref e)) if e.name().as_ref() == b"Linear" => break,
            Ok(Event::Eof) => break,
            Err(e) => {
                return Err(RitcherError::InternalError(format!(
                    "VAST XML parse error in Linear: {}",
                    e
                )));
            }
            _ => {}
        }
    }

    Ok(LinearAd {
        duration,
        media_files,
        tracking_events,
    })
}

/// Parse <MediaFiles> element
fn parse_media_files(reader: &mut Reader<&[u8]>) -> Result<Vec<MediaFile>> {
    let mut files = Vec::new();

    loop {
        match reader.read_event() {
            Ok(Event::Start(ref e)) if e.name().as_ref() == b"MediaFile" => {
                let delivery = get_attr(e, "delivery").unwrap_or_default();
                let mime_type = get_attr(e, "type").unwrap_or_default();
                let width = get_attr(e, "width")
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(0);
                let height = get_attr(e, "height")
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(0);
                let bitrate = get_attr(e, "bitrate").and_then(|s| s.parse().ok());
                let codec = get_attr(e, "codec");

                let url = read_text(reader, "MediaFile")?.trim().to_string();

                files.push(MediaFile {
                    url,
                    delivery,
                    mime_type,
                    width,
                    height,
                    bitrate,
                    codec,
                });
            }
            Ok(Event::End(ref e)) if e.name().as_ref() == b"MediaFiles" => break,
            Ok(Event::Eof) => break,
            Err(e) => {
                return Err(RitcherError::InternalError(format!(
                    "VAST XML parse error in MediaFiles: {}",
                    e
                )));
            }
            _ => {}
        }
    }

    Ok(files)
}

/// Parse <TrackingEvents> element
fn parse_tracking_events(reader: &mut Reader<&[u8]>) -> Result<Vec<TrackingEvent>> {
    let mut events = Vec::new();

    loop {
        match reader.read_event() {
            Ok(Event::Start(ref e)) if e.name().as_ref() == b"Tracking" => {
                let event = get_attr(e, "event").unwrap_or_default();
                let url = read_text(reader, "Tracking")?.trim().to_string();
                events.push(TrackingEvent { event, url });
            }
            Ok(Event::End(ref e)) if e.name().as_ref() == b"TrackingEvents" => break,
            Ok(Event::Eof) => break,
            Err(e) => {
                return Err(RitcherError::InternalError(format!(
                    "VAST XML parse error in TrackingEvents: {}",
                    e
                )));
            }
            _ => {}
        }
    }

    Ok(events)
}

/// Parse VAST duration format "HH:MM:SS" or "HH:MM:SS.mmm" to seconds
fn parse_duration(duration: &str) -> f32 {
    let parts: Vec<&str> = duration.trim().split(':').collect();
    match parts.len() {
        3 => {
            let hours: f32 = parts[0].parse().unwrap_or(0.0);
            let minutes: f32 = parts[1].parse().unwrap_or(0.0);
            let seconds: f32 = parts[2].parse().unwrap_or(0.0);
            hours * 3600.0 + minutes * 60.0 + seconds
        }
        _ => {
            warn!("Invalid VAST duration format: {}", duration);
            0.0
        }
    }
}

/// Select the best media file for SSAI stitching
///
/// Prefers HLS streaming files (application/x-mpegURL) for segment-level
/// stitching, falls back to progressive MP4 if no streaming option available.
pub fn select_best_media_file(media_files: &[MediaFile]) -> Option<&MediaFile> {
    // Prefer HLS streaming for segment-level ad insertion
    let hls = media_files
        .iter()
        .find(|f| f.mime_type == "application/x-mpegURL");
    if hls.is_some() {
        return hls;
    }

    // Fallback: progressive MP4 with highest bitrate
    let mut progressive: Vec<&MediaFile> = media_files
        .iter()
        .filter(|f| f.delivery == "progressive" && f.mime_type == "video/mp4")
        .collect();
    progressive.sort_by(|a, b| b.bitrate.cmp(&a.bitrate));
    progressive.first().copied()
}

/// Read text content from current element, handling CDATA
fn read_text(reader: &mut Reader<&[u8]>, end_tag: &str) -> Result<String> {
    let mut text = String::new();
    let end_tag_bytes = end_tag.as_bytes();

    loop {
        match reader.read_event() {
            Ok(Event::Text(e)) => {
                text.push_str(&e.unescape().unwrap_or_default());
            }
            Ok(Event::CData(e)) => {
                text.push_str(std::str::from_utf8(&e).unwrap_or_default());
            }
            Ok(Event::End(ref e)) if e.name().as_ref() == end_tag_bytes => break,
            Ok(Event::Eof) => break,
            Err(e) => {
                return Err(RitcherError::InternalError(format!(
                    "VAST XML read error: {}",
                    e
                )));
            }
            _ => {}
        }
    }

    Ok(text.trim().to_string())
}

/// Get attribute value from an XML element
fn get_attr(e: &quick_xml::events::BytesStart, name: &str) -> Option<String> {
    e.attributes()
        .filter_map(|a| a.ok())
        .find(|a| a.key.as_ref() == name.as_bytes())
        .and_then(|a| String::from_utf8(a.value.to_vec()).ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    const VAST_INLINE: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<VAST version="3.0">
  <Ad id="ad-001">
    <InLine>
      <AdSystem>Test Adserver</AdSystem>
      <AdTitle>Test Ad</AdTitle>
      <Impression>http://example.com/impression</Impression>
      <Creatives>
        <Creative id="creative-001">
          <Linear>
            <Duration>00:00:15</Duration>
            <TrackingEvents>
              <Tracking event="start">http://example.com/start</Tracking>
              <Tracking event="complete">http://example.com/complete</Tracking>
            </TrackingEvents>
            <MediaFiles>
              <MediaFile delivery="progressive" type="video/mp4" width="1280" height="720" bitrate="2000" codec="H.264">
                https://example.com/ad.mp4
              </MediaFile>
              <MediaFile delivery="streaming" type="application/x-mpegURL" width="1280" height="720">
                https://example.com/ad.m3u8
              </MediaFile>
            </MediaFiles>
          </Linear>
        </Creative>
      </Creatives>
    </InLine>
  </Ad>
</VAST>"#;

    const VAST_WRAPPER: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<VAST version="3.0">
  <Ad id="wrapper-001">
    <Wrapper>
      <AdSystem>Wrapper Server</AdSystem>
      <VASTAdTagURI><![CDATA[http://example.com/vast-inline.xml]]></VASTAdTagURI>
      <Impression>http://example.com/wrapper-impression</Impression>
    </Wrapper>
  </Ad>
</VAST>"#;

    const VAST_EMPTY: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<VAST version="3.0">
</VAST>"#;

    #[test]
    fn test_parse_inline_ad() {
        let result = parse_vast(VAST_INLINE).unwrap();

        assert_eq!(result.version, "3.0");
        assert_eq!(result.ads.len(), 1);

        let ad = &result.ads[0];
        assert_eq!(ad.id, "ad-001");

        match &ad.ad_type {
            VastAdType::InLine(inline) => {
                assert_eq!(inline.ad_system, "Test Adserver");
                assert_eq!(inline.ad_title, "Test Ad");
                assert_eq!(inline.impression_urls.len(), 1);
                assert_eq!(inline.creatives.len(), 1);

                let creative = &inline.creatives[0];
                assert_eq!(creative.id, "creative-001");

                let linear = creative.linear.as_ref().unwrap();
                assert_eq!(linear.duration, 15.0);
                assert_eq!(linear.tracking_events.len(), 2);
                assert_eq!(linear.media_files.len(), 2);

                let mp4 = &linear.media_files[0];
                assert_eq!(mp4.delivery, "progressive");
                assert_eq!(mp4.mime_type, "video/mp4");
                assert_eq!(mp4.width, 1280);
                assert_eq!(mp4.height, 720);
                assert_eq!(mp4.bitrate, Some(2000));
                assert_eq!(mp4.url, "https://example.com/ad.mp4");

                let hls = &linear.media_files[1];
                assert_eq!(hls.delivery, "streaming");
                assert_eq!(hls.mime_type, "application/x-mpegURL");
            }
            _ => panic!("Expected InLine ad"),
        }
    }

    #[test]
    fn test_parse_wrapper_ad() {
        let result = parse_vast(VAST_WRAPPER).unwrap();

        assert_eq!(result.ads.len(), 1);
        let ad = &result.ads[0];

        match &ad.ad_type {
            VastAdType::Wrapper(wrapper) => {
                assert_eq!(wrapper.ad_tag_uri, "http://example.com/vast-inline.xml");
                assert_eq!(wrapper.impression_urls.len(), 1);
            }
            _ => panic!("Expected Wrapper ad"),
        }
    }

    #[test]
    fn test_parse_empty_vast() {
        let result = parse_vast(VAST_EMPTY).unwrap();
        assert_eq!(result.version, "3.0");
        assert!(result.ads.is_empty());
    }

    #[test]
    fn test_parse_duration() {
        assert_eq!(parse_duration("00:00:15"), 15.0);
        assert_eq!(parse_duration("00:00:30"), 30.0);
        assert_eq!(parse_duration("00:01:00"), 60.0);
        assert_eq!(parse_duration("01:00:00"), 3600.0);
        assert_eq!(parse_duration("00:00:10.5"), 10.5);
    }

    #[test]
    fn test_select_best_media_file_prefers_hls() {
        let files = vec![
            MediaFile {
                url: "https://example.com/ad.mp4".to_string(),
                delivery: "progressive".to_string(),
                mime_type: "video/mp4".to_string(),
                width: 1280,
                height: 720,
                bitrate: Some(2000),
                codec: Some("H.264".to_string()),
            },
            MediaFile {
                url: "https://example.com/ad.m3u8".to_string(),
                delivery: "streaming".to_string(),
                mime_type: "application/x-mpegURL".to_string(),
                width: 1280,
                height: 720,
                bitrate: None,
                codec: None,
            },
        ];

        let best = select_best_media_file(&files).unwrap();
        assert_eq!(best.url, "https://example.com/ad.m3u8");
    }

    #[test]
    fn test_select_best_media_file_fallback_mp4() {
        let files = vec![MediaFile {
            url: "https://example.com/ad.mp4".to_string(),
            delivery: "progressive".to_string(),
            mime_type: "video/mp4".to_string(),
            width: 1280,
            height: 720,
            bitrate: Some(2000),
            codec: Some("H.264".to_string()),
        }];

        let best = select_best_media_file(&files).unwrap();
        assert_eq!(best.url, "https://example.com/ad.mp4");
    }
}
