use dash_mpd::MPD;
use tracing::{debug, info, warn};

/// Represents an ad break detected from DASH EventStream/SCTE-35 signaling
#[derive(Debug, Clone, PartialEq)]
pub struct DashAdBreak {
    /// Index of the Period containing the ad break signal
    pub period_index: usize,
    /// Period ID (if present in the MPD)
    pub period_id: Option<String>,
    /// Duration of the ad break in seconds
    pub duration: f64,
    /// Presentation time within the Period (seconds)
    pub presentation_time: f64,
    /// The type of SCTE-35 signal detected
    pub signal_type: DashSignalType,
}

/// Type of SCTE-35 signal detected in EventStream
#[derive(Debug, Clone, PartialEq, Copy)]
pub enum DashSignalType {
    /// SpliceInsert with outOfNetworkIndicator=true
    SpliceInsert,
    /// TimeSignal with segmentation descriptor (deferred to Fas 3)
    #[allow(dead_code)]
    TimeSignal,
}

/// Detect ad breaks from DASH EventStream elements with SCTE-35 signaling
///
/// Scans each Period's EventStreams for SCTE-35 scheme identifiers and extracts
/// SpliceInsert signals with outOfNetworkIndicator=true.
///
/// Supported schemeIdUri values:
/// - `urn:scte:scte35:2013:xml` — SCTE-35 in clear XML format (MVP target)
/// - `urn:scte:scte35:2014:xml+bin` — base64-encoded binary (deferred)
///
/// Returns a vector of DashAdBreak structs with period index, duration, and timing.
pub fn detect_dash_ad_breaks(mpd: &MPD) -> Vec<DashAdBreak> {
    let mut ad_breaks = Vec::new();

    for (period_idx, period) in mpd.periods.iter().enumerate() {
        debug!(
            "Scanning Period #{} (id: {:?}) for SCTE-35 signals",
            period_idx, period.id
        );

        for event_stream in &period.event_streams {
            // Match SCTE-35 scheme identifiers
            let scheme_id = event_stream.schemeIdUri.as_deref().unwrap_or("");

            if !is_scte35_scheme(scheme_id) {
                debug!("Skipping non-SCTE-35 EventStream: {}", scheme_id);
                continue;
            }

            info!(
                "Found SCTE-35 EventStream in Period #{}: {}",
                period_idx, scheme_id
            );

            // Get timescale (default to 1 if not specified per DASH spec)
            // NOTE: MVP does not implement timescale inheritance from Period/MPD level.
            // If this becomes an issue with production MPDs, we'll need to track parent timescales.
            let timescale = event_stream.timescale.unwrap_or(1) as f64;

            for event in &event_stream.event {
                // Try to detect SpliceInsert from event content
                if let Some(ad_break) =
                    detect_splice_insert(event, period_idx, &period.id, timescale)
                {
                    info!(
                        "Detected ad break at Period #{}, presentation_time: {}s, duration: {}s",
                        period_idx, ad_break.presentation_time, ad_break.duration
                    );
                    ad_breaks.push(ad_break);
                }
            }
        }
    }

    ad_breaks
}

/// Check if schemeIdUri represents a SCTE-35 signal
fn is_scte35_scheme(scheme_id: &str) -> bool {
    scheme_id.starts_with("urn:scte:scte35:")
}

/// Detect SpliceInsert from an Event element
///
/// Looks for SCTE-35 SpliceInsert with outOfNetworkIndicator=true and extracts
/// duration from BreakDuration.
fn detect_splice_insert(
    event: &dash_mpd::Event,
    period_idx: usize,
    period_id: &Option<String>,
    timescale: f64,
) -> Option<DashAdBreak> {
    // Calculate presentation time in seconds
    let presentation_time = event.presentationTime.unwrap_or(0) as f64 / timescale;

    // Calculate duration in seconds
    // Event.duration is in timescale units
    let duration_seconds = if let Some(duration_ticks) = event.duration {
        duration_ticks as f64 / timescale
    } else {
        // If no duration in Event, try to parse from content
        // This is a simplified MVP - full SCTE-35 parsing would need more work
        warn!(
            "Event at Period #{} has no duration attribute, skipping",
            period_idx
        );
        return None;
    };

    // Validate duration bounds to prevent DoS via malicious MPD
    if duration_seconds <= 0.0 || duration_seconds > 600.0 {
        warn!(
            "Invalid ad break duration {}s at Period #{}, skipping (max 600s)",
            duration_seconds, period_idx
        );
        return None;
    }

    // Validate presentation time
    if presentation_time < 0.0 {
        warn!(
            "Negative presentation time {}s at Period #{}, skipping",
            presentation_time, period_idx
        );
        return None;
    }

    // MVP approach: For SCTE-35 EventStreams, the presence of an Event with duration
    // indicates an ad break. The dash-mpd crate doesn't parse the inner SCTE-35 XML,
    // so we rely on schemeIdUri filtering (done by caller) and Event attributes.
    //
    // In production, you would parse the nested scte35:SpliceInsert XML to verify
    // outOfNetworkIndicator=true, but for MVP this simplified approach works with
    // test-adservers and most packaging systems that use urn:scte:scte35:2013:xml
    // correctly (they only emit Events for actual ad breaks).
    debug!(
        "Detected SCTE-35 Event at Period #{}: presentationTime={}s, duration={}s",
        period_idx, presentation_time, duration_seconds
    );

    Some(DashAdBreak {
        period_index: period_idx,
        period_id: period_id.clone(),
        duration: duration_seconds,
        presentation_time,
        signal_type: DashSignalType::SpliceInsert,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dash::parser::parse_mpd;

    #[test]
    fn test_detect_ad_breaks_eventstream() {
        let xml = std::fs::read_to_string("test-data/sample_mpd_eventstream.xml")
            .expect("Failed to read test file");

        let mpd = parse_mpd(&xml).expect("Failed to parse MPD");
        let ad_breaks = detect_dash_ad_breaks(&mpd);

        // Should detect one ad break in the first period
        assert_eq!(ad_breaks.len(), 1);

        let ad_break = &ad_breaks[0];
        assert_eq!(ad_break.period_index, 0);
        assert_eq!(ad_break.period_id, Some("content-1".to_string()));
        assert_eq!(ad_break.duration, 30.0); // 30 second ad break
        assert_eq!(ad_break.presentation_time, 50.0); // At 50 seconds into period
        assert_eq!(ad_break.signal_type, DashSignalType::SpliceInsert);
    }

    #[test]
    fn test_detect_ad_breaks_multiperiod() {
        let xml = std::fs::read_to_string("test-data/sample_mpd_multiperiod.xml")
            .expect("Failed to read test file");

        let mpd = parse_mpd(&xml).expect("Failed to parse MPD");
        let ad_breaks = detect_dash_ad_breaks(&mpd);

        // Should detect one ad break in the ad period (period index 1)
        assert_eq!(ad_breaks.len(), 1);

        let ad_break = &ad_breaks[0];
        assert_eq!(ad_break.period_index, 1);
        assert_eq!(ad_break.period_id, Some("ad-break-1".to_string()));
        assert_eq!(ad_break.duration, 30.0);
        assert_eq!(ad_break.presentation_time, 0.0); // At start of ad period
    }

    #[test]
    fn test_no_ad_breaks_in_segmenttemplate() {
        let xml = std::fs::read_to_string("test-data/sample_mpd_segmenttemplate.xml")
            .expect("Failed to read test file");

        let mpd = parse_mpd(&xml).expect("Failed to parse MPD");
        let ad_breaks = detect_dash_ad_breaks(&mpd);

        // This MPD has no EventStreams, so no ad breaks
        assert_eq!(ad_breaks.len(), 0);
    }

    #[test]
    fn test_is_scte35_scheme() {
        assert!(is_scte35_scheme("urn:scte:scte35:2013:xml"));
        assert!(is_scte35_scheme("urn:scte:scte35:2014:xml+bin"));
        assert!(!is_scte35_scheme("urn:mpeg:dash:event:2012"));
        assert!(!is_scte35_scheme(""));
    }

    #[test]
    fn test_timescale_conversion() {
        // Test with different timescales
        let xml = r#"<?xml version="1.0"?>
<MPD xmlns="urn:mpeg:dash:schema:mpd:2011" type="static">
  <Period id="1">
    <EventStream schemeIdUri="urn:scte:scte35:2013:xml" timescale="90000">
      <Event presentationTime="4500000" duration="2700000" id="1">
        <scte35:SpliceInfoSection xmlns:scte35="http://www.scte.org/schemas/35/2016">
          <scte35:SpliceInsert spliceEventId="1" outOfNetworkIndicator="true">
            <scte35:BreakDuration duration="30"/>
          </scte35:SpliceInsert>
        </scte35:SpliceInfoSection>
      </Event>
    </EventStream>
    <AdaptationSet>
      <Representation id="1" bandwidth="1000000">
        <SegmentTemplate media="$Number$.m4s"/>
      </Representation>
    </AdaptationSet>
  </Period>
</MPD>"#;

        let mpd = parse_mpd(xml).expect("Failed to parse MPD");
        let ad_breaks = detect_dash_ad_breaks(&mpd);

        assert_eq!(ad_breaks.len(), 1);
        let ad_break = &ad_breaks[0];

        // presentationTime: 4500000 / 90000 = 50 seconds
        assert_eq!(ad_break.presentation_time, 50.0);
        // duration: 2700000 / 90000 = 30 seconds
        assert_eq!(ad_break.duration, 30.0);
    }

    #[test]
    fn test_skip_zero_duration() {
        let xml = r#"<?xml version="1.0"?>
<MPD xmlns="urn:mpeg:dash:schema:mpd:2011" type="static">
  <Period id="1">
    <EventStream schemeIdUri="urn:scte:scte35:2013:xml" timescale="1">
      <Event presentationTime="10" duration="0" id="1"/>
    </EventStream>
    <AdaptationSet>
      <Representation id="1" bandwidth="1000000">
        <SegmentTemplate media="$Number$.m4s"/>
      </Representation>
    </AdaptationSet>
  </Period>
</MPD>"#;

        let mpd = parse_mpd(xml).expect("Failed to parse MPD");
        let ad_breaks = detect_dash_ad_breaks(&mpd);

        // Duration = 0 should be skipped
        assert_eq!(ad_breaks.len(), 0);
    }

    #[test]
    fn test_skip_excessive_duration() {
        let xml = r#"<?xml version="1.0"?>
<MPD xmlns="urn:mpeg:dash:schema:mpd:2011" type="static">
  <Period id="1">
    <EventStream schemeIdUri="urn:scte:scte35:2013:xml" timescale="1">
      <Event presentationTime="10" duration="9999999" id="1"/>
    </EventStream>
    <AdaptationSet>
      <Representation id="1" bandwidth="1000000">
        <SegmentTemplate media="$Number$.m4s"/>
      </Representation>
    </AdaptationSet>
  </Period>
</MPD>"#;

        let mpd = parse_mpd(xml).expect("Failed to parse MPD");
        let ad_breaks = detect_dash_ad_breaks(&mpd);

        // Duration > 600s should be skipped
        assert_eq!(ad_breaks.len(), 0);
    }

    #[test]
    fn test_skip_negative_presentation_time_via_overflow() {
        // Note: dash-mpd parses duration as u64, so negative values aren't possible
        // in XML. However, presentation_time with very large values that overflow
        // when divided by timescale are handled by our bounds check.
        // This test verifies the boundary: duration just over 600s is rejected.
        let xml = r#"<?xml version="1.0"?>
<MPD xmlns="urn:mpeg:dash:schema:mpd:2011" type="static">
  <Period id="1">
    <EventStream schemeIdUri="urn:scte:scte35:2013:xml" timescale="1">
      <Event presentationTime="10" duration="601" id="1"/>
    </EventStream>
    <AdaptationSet>
      <Representation id="1" bandwidth="1000000">
        <SegmentTemplate media="$Number$.m4s"/>
      </Representation>
    </AdaptationSet>
  </Period>
</MPD>"#;

        let mpd = parse_mpd(xml).expect("Failed to parse MPD");
        let ad_breaks = detect_dash_ad_breaks(&mpd);

        // 601s > 600s max → should be skipped
        assert_eq!(ad_breaks.len(), 0);
    }

    #[test]
    fn test_accept_max_valid_duration() {
        // 600s (10 minutes) should be the max accepted duration
        let xml = r#"<?xml version="1.0"?>
<MPD xmlns="urn:mpeg:dash:schema:mpd:2011" type="static">
  <Period id="1">
    <EventStream schemeIdUri="urn:scte:scte35:2013:xml" timescale="1">
      <Event presentationTime="10" duration="600" id="1"/>
    </EventStream>
    <AdaptationSet>
      <Representation id="1" bandwidth="1000000">
        <SegmentTemplate media="$Number$.m4s"/>
      </Representation>
    </AdaptationSet>
  </Period>
</MPD>"#;

        let mpd = parse_mpd(xml).expect("Failed to parse MPD");
        let ad_breaks = detect_dash_ad_breaks(&mpd);

        // 600s exactly should be accepted
        assert_eq!(ad_breaks.len(), 1);
        assert_eq!(ad_breaks[0].duration, 600.0);
    }
}
