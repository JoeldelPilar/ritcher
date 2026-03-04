use crate::error::{Result, RitcherError};
use quick_xml::events::Event;
use quick_xml::reader::Reader;
use tracing::info;

use super::helpers::{get_attr, parse_duration, read_text};
use super::types::{
    Creative, InLineAd, LinearAd, MediaFile, TrackingEvent, VastAd, VastAdType, VastResponse,
    Verification, VerificationTrackingEvent, WrapperAd,
};

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
    let mut verifications = Vec::new();

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
            Ok(Event::Start(ref e)) if e.name().as_ref() == b"AdVerifications" => {
                verifications = parse_ad_verifications(reader)?;
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
        verifications,
    })
}

/// Parse <Wrapper> element
fn parse_wrapper(reader: &mut Reader<&[u8]>) -> Result<WrapperAd> {
    let mut ad_tag_uri = String::new();
    let mut impression_urls = Vec::new();
    let mut tracking_events = Vec::new();
    let mut verifications = Vec::new();

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
            Ok(Event::Start(ref e)) if e.name().as_ref() == b"AdVerifications" => {
                verifications = parse_ad_verifications(reader)?;
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
        verifications,
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
pub(crate) fn parse_tracking_events(reader: &mut Reader<&[u8]>) -> Result<Vec<TrackingEvent>> {
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

/// Parse `<AdVerifications>` element containing one or more `<Verification>` children
fn parse_ad_verifications(reader: &mut Reader<&[u8]>) -> Result<Vec<Verification>> {
    let mut verifications = Vec::new();

    loop {
        match reader.read_event() {
            Ok(Event::Start(ref e)) if e.name().as_ref() == b"Verification" => {
                let vendor = get_attr(e, "vendor");
                let api_framework = get_attr(e, "apiFramework");
                let verification = parse_verification(reader, vendor, api_framework)?;
                verifications.push(verification);
            }
            Ok(Event::End(ref e)) if e.name().as_ref() == b"AdVerifications" => break,
            Ok(Event::Eof) => break,
            Err(e) => {
                return Err(RitcherError::InternalError(format!(
                    "VAST XML parse error in AdVerifications: {}",
                    e
                )));
            }
            _ => {}
        }
    }

    Ok(verifications)
}

/// Parse a single `<Verification>` element
fn parse_verification(
    reader: &mut Reader<&[u8]>,
    vendor: Option<String>,
    api_framework: Option<String>,
) -> Result<Verification> {
    let mut javascript_resource_url = None;
    let mut parameters = None;
    let mut tracking_events = Vec::new();

    loop {
        match reader.read_event() {
            Ok(Event::Start(ref e)) if e.name().as_ref() == b"JavaScriptResource" => {
                let url = read_text(reader, "JavaScriptResource")?;
                if !url.is_empty() {
                    javascript_resource_url = Some(url);
                }
            }
            Ok(Event::Start(ref e)) if e.name().as_ref() == b"VerificationParameters" => {
                let params = read_text(reader, "VerificationParameters")?;
                if !params.is_empty() {
                    parameters = Some(params);
                }
            }
            Ok(Event::Start(ref e)) if e.name().as_ref() == b"TrackingEvents" => {
                tracking_events = parse_verification_tracking_events(reader)?;
            }
            Ok(Event::End(ref e)) if e.name().as_ref() == b"Verification" => break,
            Ok(Event::Eof) => break,
            Err(e) => {
                return Err(RitcherError::InternalError(format!(
                    "VAST XML parse error in Verification: {}",
                    e
                )));
            }
            _ => {}
        }
    }

    Ok(Verification {
        vendor,
        javascript_resource_url,
        api_framework,
        parameters,
        tracking_events,
    })
}

/// Parse `<TrackingEvents>` within a `<Verification>` node
fn parse_verification_tracking_events(
    reader: &mut Reader<&[u8]>,
) -> Result<Vec<VerificationTrackingEvent>> {
    let mut events = Vec::new();

    loop {
        match reader.read_event() {
            Ok(Event::Start(ref e)) if e.name().as_ref() == b"Tracking" => {
                let event = get_attr(e, "event").unwrap_or_default();
                let uri = read_text(reader, "Tracking")?.trim().to_string();
                events.push(VerificationTrackingEvent { event, uri });
            }
            Ok(Event::End(ref e)) if e.name().as_ref() == b"TrackingEvents" => break,
            Ok(Event::Eof) => break,
            Err(e) => {
                return Err(RitcherError::InternalError(format!(
                    "VAST XML parse error in Verification TrackingEvents: {}",
                    e
                )));
            }
            _ => {}
        }
    }

    Ok(events)
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

    // -- Malicious / edge case VAST inputs --

    #[test]
    fn malformed_xml_returns_error() {
        let xml = r#"<VAST version="3.0"><Ad id="1"><InLine><unclosed"#;
        let result = parse_vast(xml);
        assert!(result.is_err(), "Malformed XML should produce an error");
    }

    #[test]
    fn xxe_entity_expansion_ignored() {
        // XML external entity attack -- quick-xml does not resolve external entities
        let xml = r#"<?xml version="1.0"?>
<!DOCTYPE foo [
  <!ENTITY xxe SYSTEM "file:///etc/passwd">
]>
<VAST version="3.0">
  <Ad id="xxe-test">
    <InLine>
      <AdSystem>&xxe;</AdSystem>
      <AdTitle>XXE Test</AdTitle>
      <Creatives></Creatives>
    </InLine>
  </Ad>
</VAST>"#;
        // quick-xml does not expand external entities by default
        // It should either error or return the entity reference as-is
        let result = parse_vast(xml);
        if let Ok(resp) = result
            && let Some(ad) = resp.ads.first()
            && let VastAdType::InLine(inline) = &ad.ad_type
        {
            assert!(
                !inline.ad_system.contains("root:"),
                "XXE entity should not be expanded"
            );
        }
        // Either an error or safe non-expansion is acceptable
    }

    #[test]
    fn empty_vast_body() {
        let xml = r#"<VAST version="4.0"></VAST>"#;
        let result = parse_vast(xml).unwrap();
        assert!(result.ads.is_empty());
        assert_eq!(result.version, "4.0");
    }

    #[test]
    fn ad_without_inline_or_wrapper_skipped() {
        let xml = r#"<VAST version="3.0">
  <Ad id="empty-ad">
  </Ad>
</VAST>"#;
        let result = parse_vast(xml).unwrap();
        assert!(
            result.ads.is_empty(),
            "Ad without InLine or Wrapper should be skipped"
        );
    }

    #[test]
    fn missing_media_files_produces_empty_creative() {
        let xml = r#"<VAST version="3.0">
  <Ad id="no-media">
    <InLine>
      <AdSystem>Test</AdSystem>
      <AdTitle>No Media</AdTitle>
      <Creatives>
        <Creative id="c1">
          <Linear>
            <Duration>00:00:15</Duration>
            <MediaFiles></MediaFiles>
          </Linear>
        </Creative>
      </Creatives>
    </InLine>
  </Ad>
</VAST>"#;
        let result = parse_vast(xml).unwrap();
        let ad = &result.ads[0];
        if let VastAdType::InLine(inline) = &ad.ad_type {
            let linear = inline.creatives[0].linear.as_ref().unwrap();
            assert!(
                linear.media_files.is_empty(),
                "Empty MediaFiles should produce empty vec"
            );
        }
    }

    #[test]
    fn whitespace_around_urls_trimmed() {
        let xml = r#"<VAST version="3.0">
  <Ad id="ws-test">
    <InLine>
      <AdSystem>Test</AdSystem>
      <AdTitle>Whitespace Test</AdTitle>
      <Impression>
        http://example.com/imp
      </Impression>
      <Creatives>
        <Creative id="c1">
          <Linear>
            <Duration>00:00:10</Duration>
            <MediaFiles>
              <MediaFile delivery="progressive" type="video/mp4" width="640" height="360">
                  https://example.com/ad.mp4
              </MediaFile>
            </MediaFiles>
          </Linear>
        </Creative>
      </Creatives>
    </InLine>
  </Ad>
</VAST>"#;
        let result = parse_vast(xml).unwrap();
        if let VastAdType::InLine(inline) = &result.ads[0].ad_type {
            assert_eq!(inline.impression_urls[0], "http://example.com/imp");
            let url = &inline.creatives[0].linear.as_ref().unwrap().media_files[0].url;
            assert_eq!(url, "https://example.com/ad.mp4");
        }
    }

    #[test]
    fn multiple_ads_all_parsed() {
        let xml = r#"<VAST version="3.0">
  <Ad id="ad-1">
    <InLine>
      <AdSystem>Server</AdSystem>
      <AdTitle>First</AdTitle>
      <Creatives></Creatives>
    </InLine>
  </Ad>
  <Ad id="ad-2">
    <InLine>
      <AdSystem>Server</AdSystem>
      <AdTitle>Second</AdTitle>
      <Creatives></Creatives>
    </InLine>
  </Ad>
  <Ad id="ad-3">
    <InLine>
      <AdSystem>Server</AdSystem>
      <AdTitle>Third</AdTitle>
      <Creatives></Creatives>
    </InLine>
  </Ad>
</VAST>"#;
        let result = parse_vast(xml).unwrap();
        assert_eq!(result.ads.len(), 3);
        assert_eq!(result.ads[0].id, "ad-1");
        assert_eq!(result.ads[2].id, "ad-3");
    }

    #[test]
    fn cdata_in_vast_ad_tag_uri() {
        let xml = r#"<VAST version="3.0">
  <Ad id="cdata-test">
    <Wrapper>
      <VASTAdTagURI><![CDATA[https://ads.example.com/vast?cb=12345&format=xml]]></VASTAdTagURI>
      <Impression><![CDATA[https://track.example.com/imp?a=1&b=2]]></Impression>
    </Wrapper>
  </Ad>
</VAST>"#;
        let result = parse_vast(xml).unwrap();
        if let VastAdType::Wrapper(wrapper) = &result.ads[0].ad_type {
            assert_eq!(
                wrapper.ad_tag_uri,
                "https://ads.example.com/vast?cb=12345&format=xml"
            );
            assert_eq!(
                wrapper.impression_urls[0],
                "https://track.example.com/imp?a=1&b=2"
            );
        }
    }

    // -- OMID <AdVerifications> tests --

    const VAST_INLINE_WITH_VERIFICATIONS: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<VAST version="4.1">
  <Ad id="omid-ad">
    <InLine>
      <AdSystem>TestAds</AdSystem>
      <AdTitle>OMID Ad</AdTitle>
      <Impression>http://example.com/impression</Impression>
      <AdVerifications>
        <Verification vendor="doubleverify.com-omid" apiFramework="omid">
          <JavaScriptResource><![CDATA[https://cdn.doubleverify.com/dvtp_src.js]]></JavaScriptResource>
          <VerificationParameters><![CDATA[ctx=123&cmp=abc]]></VerificationParameters>
          <TrackingEvents>
            <Tracking event="verificationNotExecuted">https://verify.example.com/failed</Tracking>
          </TrackingEvents>
        </Verification>
        <Verification vendor="ias.com-omid" apiFramework="omid">
          <JavaScriptResource>https://cdn.ias.com/omid.js</JavaScriptResource>
        </Verification>
      </AdVerifications>
      <Creatives>
        <Creative id="c1">
          <Linear>
            <Duration>00:00:30</Duration>
            <MediaFiles>
              <MediaFile delivery="progressive" type="video/mp4" width="1280" height="720">
                https://example.com/ad.mp4
              </MediaFile>
            </MediaFiles>
          </Linear>
        </Creative>
      </Creatives>
    </InLine>
  </Ad>
</VAST>"#;

    #[test]
    fn test_parse_inline_with_ad_verifications() {
        let result = parse_vast(VAST_INLINE_WITH_VERIFICATIONS).unwrap();
        assert_eq!(result.ads.len(), 1);

        let ad = &result.ads[0];
        if let VastAdType::InLine(inline) = &ad.ad_type {
            assert_eq!(inline.verifications.len(), 2);

            // First verification: DoubleVerify
            let dv = &inline.verifications[0];
            assert_eq!(dv.vendor.as_deref(), Some("doubleverify.com-omid"));
            assert_eq!(dv.api_framework.as_deref(), Some("omid"));
            assert_eq!(
                dv.javascript_resource_url.as_deref(),
                Some("https://cdn.doubleverify.com/dvtp_src.js")
            );
            assert_eq!(dv.parameters.as_deref(), Some("ctx=123&cmp=abc"));
            assert_eq!(dv.tracking_events.len(), 1);
            assert_eq!(dv.tracking_events[0].event, "verificationNotExecuted");
            assert_eq!(
                dv.tracking_events[0].uri,
                "https://verify.example.com/failed"
            );

            // Second verification: IAS (minimal -- no params, no tracking)
            let ias = &inline.verifications[1];
            assert_eq!(ias.vendor.as_deref(), Some("ias.com-omid"));
            assert_eq!(ias.api_framework.as_deref(), Some("omid"));
            assert_eq!(
                ias.javascript_resource_url.as_deref(),
                Some("https://cdn.ias.com/omid.js")
            );
            assert!(ias.parameters.is_none());
            assert!(ias.tracking_events.is_empty());
        } else {
            panic!("Expected InLine ad");
        }
    }

    #[test]
    fn test_parse_inline_without_verifications() {
        // The standard VAST_INLINE constant has no <AdVerifications>
        let result = parse_vast(VAST_INLINE).unwrap();
        if let VastAdType::InLine(inline) = &result.ads[0].ad_type {
            assert!(
                inline.verifications.is_empty(),
                "InLine without AdVerifications should have empty vec"
            );
        } else {
            panic!("Expected InLine ad");
        }
    }

    #[test]
    fn test_parse_wrapper_with_verifications() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<VAST version="4.1">
  <Ad id="wrapper-v">
    <Wrapper>
      <VASTAdTagURI>http://example.com/vast-inline.xml</VASTAdTagURI>
      <Impression>http://example.com/wrapper-imp</Impression>
      <AdVerifications>
        <Verification vendor="moat.com-omid" apiFramework="omid">
          <JavaScriptResource>https://cdn.moat.com/omid.js</JavaScriptResource>
          <VerificationParameters>moat_partner=abc123</VerificationParameters>
        </Verification>
      </AdVerifications>
    </Wrapper>
  </Ad>
</VAST>"#;
        let result = parse_vast(xml).unwrap();
        if let VastAdType::Wrapper(wrapper) = &result.ads[0].ad_type {
            assert_eq!(wrapper.verifications.len(), 1);
            let v = &wrapper.verifications[0];
            assert_eq!(v.vendor.as_deref(), Some("moat.com-omid"));
            assert_eq!(
                v.javascript_resource_url.as_deref(),
                Some("https://cdn.moat.com/omid.js")
            );
            assert_eq!(v.parameters.as_deref(), Some("moat_partner=abc123"));
        } else {
            panic!("Expected Wrapper ad");
        }
    }

    #[test]
    fn test_parse_wrapper_without_verifications() {
        let result = parse_vast(VAST_WRAPPER).unwrap();
        if let VastAdType::Wrapper(wrapper) = &result.ads[0].ad_type {
            assert!(
                wrapper.verifications.is_empty(),
                "Wrapper without AdVerifications should have empty vec"
            );
        } else {
            panic!("Expected Wrapper ad");
        }
    }

    #[test]
    fn test_parse_empty_ad_verifications() {
        let xml = r#"<VAST version="4.1">
  <Ad id="empty-v">
    <InLine>
      <AdSystem>Test</AdSystem>
      <AdTitle>Empty Verifications</AdTitle>
      <AdVerifications></AdVerifications>
      <Creatives></Creatives>
    </InLine>
  </Ad>
</VAST>"#;
        let result = parse_vast(xml).unwrap();
        if let VastAdType::InLine(inline) = &result.ads[0].ad_type {
            assert!(
                inline.verifications.is_empty(),
                "Empty AdVerifications should produce empty vec"
            );
        }
    }

    #[test]
    fn test_parse_verification_without_optional_fields() {
        let xml = r#"<VAST version="4.1">
  <Ad id="minimal-v">
    <InLine>
      <AdSystem>Test</AdSystem>
      <AdTitle>Minimal Verification</AdTitle>
      <AdVerifications>
        <Verification>
          <JavaScriptResource>https://example.com/verify.js</JavaScriptResource>
        </Verification>
      </AdVerifications>
      <Creatives></Creatives>
    </InLine>
  </Ad>
</VAST>"#;
        let result = parse_vast(xml).unwrap();
        if let VastAdType::InLine(inline) = &result.ads[0].ad_type {
            assert_eq!(inline.verifications.len(), 1);
            let v = &inline.verifications[0];
            assert!(v.vendor.is_none(), "vendor should be None when not set");
            assert!(
                v.api_framework.is_none(),
                "apiFramework should be None when not set"
            );
            assert_eq!(
                v.javascript_resource_url.as_deref(),
                Some("https://example.com/verify.js")
            );
            assert!(v.parameters.is_none());
            assert!(v.tracking_events.is_empty());
        }
    }

    #[test]
    fn test_parse_verification_multiple_tracking_events() {
        let xml = r#"<VAST version="4.1">
  <Ad id="multi-track">
    <InLine>
      <AdSystem>Test</AdSystem>
      <AdTitle>Multi Tracking</AdTitle>
      <AdVerifications>
        <Verification vendor="test-vendor" apiFramework="omid">
          <JavaScriptResource>https://example.com/verify.js</JavaScriptResource>
          <TrackingEvents>
            <Tracking event="verificationNotExecuted">https://example.com/notExec</Tracking>
            <Tracking event="loaded">https://example.com/loaded</Tracking>
          </TrackingEvents>
        </Verification>
      </AdVerifications>
      <Creatives></Creatives>
    </InLine>
  </Ad>
</VAST>"#;
        let result = parse_vast(xml).unwrap();
        if let VastAdType::InLine(inline) = &result.ads[0].ad_type {
            let v = &inline.verifications[0];
            assert_eq!(v.tracking_events.len(), 2);
            assert_eq!(v.tracking_events[0].event, "verificationNotExecuted");
            assert_eq!(v.tracking_events[0].uri, "https://example.com/notExec");
            assert_eq!(v.tracking_events[1].event, "loaded");
            assert_eq!(v.tracking_events[1].uri, "https://example.com/loaded");
        }
    }

    #[test]
    fn test_parse_verification_cdata_in_parameters() {
        let xml = r#"<VAST version="4.1">
  <Ad id="cdata-params">
    <InLine>
      <AdSystem>Test</AdSystem>
      <AdTitle>CDATA Params</AdTitle>
      <AdVerifications>
        <Verification vendor="dv" apiFramework="omid">
          <JavaScriptResource>https://cdn.dv.com/script.js</JavaScriptResource>
          <VerificationParameters><![CDATA[key1=val1&key2=val2&special=<>"']]></VerificationParameters>
        </Verification>
      </AdVerifications>
      <Creatives></Creatives>
    </InLine>
  </Ad>
</VAST>"#;
        let result = parse_vast(xml).unwrap();
        if let VastAdType::InLine(inline) = &result.ads[0].ad_type {
            let v = &inline.verifications[0];
            assert_eq!(
                v.parameters.as_deref(),
                Some("key1=val1&key2=val2&special=<>\"'")
            );
        }
    }
}
