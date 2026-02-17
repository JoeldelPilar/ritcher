use crate::error::{Result, RitcherError};
use dash_mpd::MPD;
use tracing::info;

/// Parse DASH MPD from XML string content
pub fn parse_mpd(xml: &str) -> Result<MPD> {
    info!("Parsing DASH MPD");

    dash_mpd::parse(xml)
        .map_err(|e| {
            let error_msg = format!("Failed to parse MPD: {}", e);
            RitcherError::MpdParseError(error_msg)
        })
        .inspect(|mpd| {
            info!("Successfully parsed MPD with {} periods", mpd.periods.len());
        })
}

/// Serialize MPD back to XML string
pub fn serialize_mpd(mpd: &MPD) -> Result<String> {
    info!("Serializing DASH MPD");

    // MPD implements Display, so to_string() returns String directly
    Ok(mpd.to_string())
}

/// Compose a URL from a base and a relative path, avoiding double slashes.
///
/// If `relative` is empty, returns `base` unchanged.
/// If `relative` is an absolute URL (starts with "http"), it replaces `base`.
/// Otherwise, joins them with a single `/`.
fn compose_url(base: &str, relative: &str) -> String {
    if relative.is_empty() {
        return base.to_string();
    }
    if relative.starts_with("http") {
        return relative.to_string();
    }
    format!(
        "{}/{}",
        base.trim_end_matches('/'),
        relative.trim_start_matches('/')
    )
}

/// Rewrite DASH URLs to route through stitcher's proxy
///
/// This function rewrites BaseURL elements and SegmentTemplate URLs at various
/// levels of the MPD hierarchy to proxy through the stitcher.
///
/// DASH uses hierarchical BaseURL resolution (ISO/IEC 23009-1 §5.6.6):
/// MPD BaseURL → Period BaseURL → AdaptationSet BaseURL → Representation BaseURL
///
/// We extract the effective origin at each level before clearing parent BaseURLs,
/// then rewrite at the Representation level to absolute proxy URLs.
pub fn rewrite_dash_urls(
    mpd: &mut MPD,
    session_id: &str,
    base_url: &str,
    origin_base: &str,
) -> Result<()> {
    info!("Rewriting DASH URLs for session: {}", session_id);

    // Extract MPD-level BaseURL before clearing (hierarchical inheritance)
    let mpd_base = if !mpd.base_url.is_empty() {
        compose_url(origin_base, &mpd.base_url[0].base)
    } else {
        origin_base.to_string()
    };
    mpd.base_url.clear();

    for period in &mut mpd.periods {
        // Period inherits from MPD base
        let period_base = if !period.BaseURL.is_empty() {
            compose_url(&mpd_base, &period.BaseURL[0].base)
        } else {
            mpd_base.clone()
        };
        period.BaseURL.clear();

        for adaptation_set in &mut period.adaptations {
            // AdaptationSet inherits from Period base
            let adaptation_base = if !adaptation_set.BaseURL.is_empty() {
                compose_url(&period_base, &adaptation_set.BaseURL[0].base)
            } else {
                period_base.clone()
            };
            adaptation_set.BaseURL.clear();

            for representation in &mut adaptation_set.representations {
                // Representation inherits from AdaptationSet base
                let repr_origin = if !representation.BaseURL.is_empty() {
                    compose_url(&adaptation_base, &representation.BaseURL[0].base)
                } else {
                    adaptation_base.clone()
                };
                representation.BaseURL.clear();

                // Rewrite SegmentTemplate URLs if present
                if let Some(ref mut segment_template) = representation.SegmentTemplate {
                    rewrite_segment_template(
                        segment_template,
                        session_id,
                        base_url,
                        &repr_origin,
                    )?;
                }

                // Also check AdaptationSet-level SegmentTemplate
                if let Some(ref mut segment_template) = adaptation_set.SegmentTemplate {
                    rewrite_segment_template(
                        segment_template,
                        session_id,
                        base_url,
                        &repr_origin,
                    )?;
                }
            }
        }
    }

    Ok(())
}

/// Rewrite SegmentTemplate media and initialization URLs
fn rewrite_segment_template(
    template: &mut dash_mpd::SegmentTemplate,
    session_id: &str,
    base_url: &str,
    origin: &str,
) -> Result<()> {
    // Rewrite initialization URL
    if let Some(ref initialization) = template.initialization {
        let proxied_init = format!(
            "{}/stitch/{}/segment/{}?origin={}",
            base_url, session_id, initialization, origin
        );
        template.initialization = Some(proxied_init);
    }

    // Rewrite media template
    if let Some(ref media) = template.media {
        // For templates with $Number$ or $Time$, we keep the template but
        // wrap it in our proxy URL structure
        let proxied_media = format!(
            "{}/stitch/{}/segment/{}?origin={}",
            base_url, session_id, media, origin
        );
        template.media = Some(proxied_media);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_and_serialize_roundtrip() {
        let xml = std::fs::read_to_string("test-data/sample_mpd_segmenttemplate.xml")
            .expect("Failed to read test file");

        let mpd = parse_mpd(&xml).expect("Failed to parse MPD");
        let serialized = serialize_mpd(&mpd).expect("Failed to serialize MPD");

        // Parse again to verify it's valid XML
        let reparsed = parse_mpd(&serialized).expect("Failed to reparse serialized MPD");

        // Basic structure checks
        assert_eq!(mpd.periods.len(), reparsed.periods.len());
        assert_eq!(
            mpd.periods[0].adaptations.len(),
            reparsed.periods[0].adaptations.len()
        );
    }

    #[test]
    fn test_rewrite_segment_template_urls() {
        let xml = std::fs::read_to_string("test-data/sample_mpd_segmenttemplate.xml")
            .expect("Failed to read test file");

        let mut mpd = parse_mpd(&xml).expect("Failed to parse MPD");

        rewrite_dash_urls(
            &mut mpd,
            "test-session",
            "http://stitcher.local",
            "https://origin.example.com/video",
        )
        .expect("Failed to rewrite URLs");

        // Check that BaseURLs were cleared
        assert!(mpd.periods[0].BaseURL.is_empty());

        // Check SegmentTemplate rewriting
        let repr = &mpd.periods[0].adaptations[0].representations[0];
        if let Some(ref template) = repr.SegmentTemplate {
            if let Some(ref media) = template.media {
                assert!(media.contains("/stitch/test-session/segment/"));
                assert!(media.contains("origin="));
            }
            if let Some(ref init) = template.initialization {
                assert!(init.contains("/stitch/test-session/segment/"));
            }
        }
    }

    #[test]
    fn test_parse_multiperiod_mpd() {
        let xml = std::fs::read_to_string("test-data/sample_mpd_multiperiod.xml")
            .expect("Failed to read test file");

        let mpd = parse_mpd(&xml).expect("Failed to parse MPD");

        // Verify period structure
        assert_eq!(mpd.periods.len(), 3);
        assert_eq!(mpd.periods[0].id, Some("content-1".to_string()));
        assert_eq!(mpd.periods[1].id, Some("ad-break-1".to_string()));
        assert_eq!(mpd.periods[2].id, Some("content-2".to_string()));
    }

    #[test]
    fn test_parse_eventstream_mpd() {
        let xml = std::fs::read_to_string("test-data/sample_mpd_eventstream.xml")
            .expect("Failed to read test file");

        let mpd = parse_mpd(&xml).expect("Failed to parse MPD");

        // Verify EventStream exists
        assert_eq!(mpd.periods.len(), 2);
        assert!(!mpd.periods[0].event_streams.is_empty());

        let event_stream = &mpd.periods[0].event_streams[0];
        assert_eq!(
            event_stream.schemeIdUri,
            Some("urn:scte:scte35:2013:xml".to_string())
        );
        assert!(!event_stream.event.is_empty());
    }

    #[test]
    fn test_parse_invalid_mpd() {
        // dash-mpd is quite tolerant and will parse even minimal/invalid XML
        // This test verifies that completely malformed XML fails
        let invalid_xml = "this is not XML at all";
        let result = parse_mpd(invalid_xml);
        assert!(result.is_err());
    }

    #[test]
    fn test_rewrite_multiple_adaptation_sets() {
        let xml = r#"<?xml version="1.0"?>
<MPD xmlns="urn:mpeg:dash:schema:mpd:2011" type="static">
  <Period>
    <AdaptationSet contentType="video">
      <Representation id="v1" bandwidth="1000000">
        <SegmentTemplate media="video-$Number$.m4s" initialization="video-init.mp4"/>
      </Representation>
    </AdaptationSet>
    <AdaptationSet contentType="audio">
      <Representation id="a1" bandwidth="128000">
        <SegmentTemplate media="audio-$Number$.m4s" initialization="audio-init.mp4"/>
      </Representation>
    </AdaptationSet>
  </Period>
</MPD>"#;

        let mut mpd = parse_mpd(xml).expect("Failed to parse MPD");
        rewrite_dash_urls(&mut mpd, "test", "http://stitcher", "https://origin.example.com")
            .expect("Failed to rewrite URLs");

        // Verify both AdaptationSets were rewritten
        assert_eq!(mpd.periods[0].adaptations.len(), 2);

        // Check video SegmentTemplate
        let video_repr = &mpd.periods[0].adaptations[0].representations[0];
        if let Some(ref template) = video_repr.SegmentTemplate {
            assert!(
                template.media.as_ref().unwrap().contains("/stitch/test/segment/video-"),
                "Video media template not rewritten"
            );
            assert!(
                template.initialization.as_ref().unwrap().contains("/stitch/test/segment/video-init"),
                "Video init template not rewritten"
            );
        }

        // Check audio SegmentTemplate
        let audio_repr = &mpd.periods[0].adaptations[1].representations[0];
        if let Some(ref template) = audio_repr.SegmentTemplate {
            assert!(
                template.media.as_ref().unwrap().contains("/stitch/test/segment/audio-"),
                "Audio media template not rewritten"
            );
            assert!(
                template.initialization.as_ref().unwrap().contains("/stitch/test/segment/audio-init"),
                "Audio init template not rewritten"
            );
        }
    }

    #[test]
    fn test_compose_url_no_double_slashes() {
        // Test the compose_url helper directly
        assert_eq!(
            compose_url("https://example.com/", "path/to/file"),
            "https://example.com/path/to/file"
        );
        assert_eq!(
            compose_url("https://example.com", "/path/to/file"),
            "https://example.com/path/to/file"
        );
        assert_eq!(
            compose_url("https://example.com/", "/path/to/file"),
            "https://example.com/path/to/file"
        );
        assert_eq!(
            compose_url("https://example.com", ""),
            "https://example.com"
        );
        // Absolute URL replaces base
        assert_eq!(
            compose_url("https://example.com/old", "https://cdn.new.com/path"),
            "https://cdn.new.com/path"
        );
    }

    #[test]
    fn test_hierarchical_base_url_resolution() {
        // Verify that MPD-level BaseURL is inherited when Period has no BaseURL
        let xml = r#"<?xml version="1.0"?>
<MPD xmlns="urn:mpeg:dash:schema:mpd:2011" type="static">
  <BaseURL>https://cdn.example.com/v1/</BaseURL>
  <Period>
    <AdaptationSet>
      <Representation id="1" bandwidth="1000000">
        <SegmentTemplate media="seg-$Number$.m4s" initialization="init.mp4"/>
      </Representation>
    </AdaptationSet>
  </Period>
</MPD>"#;

        let mut mpd = parse_mpd(xml).expect("Failed to parse MPD");
        rewrite_dash_urls(
            &mut mpd,
            "sess1",
            "http://stitcher.local",
            "https://fallback.example.com",
        )
        .expect("Failed to rewrite URLs");

        // MPD BaseURL should be cleared
        assert!(mpd.base_url.is_empty());

        // SegmentTemplate should use MPD-level BaseURL (cdn.example.com/v1) as origin
        let repr = &mpd.periods[0].adaptations[0].representations[0];
        if let Some(ref template) = repr.SegmentTemplate {
            let init = template.initialization.as_ref().unwrap();
            assert!(
                init.contains("origin=https://cdn.example.com/v1"),
                "Expected MPD BaseURL in origin, got: {}",
                init
            );
        }
    }
}
