//! DASH SGAI (Server-Guided Ad Insertion) via MPD callback EventStream
//!
//! Implements the DASH event callback mechanism (ISO 23009-1, scheme
//! `urn:mpeg:dash:event:callback:2015`). When a DASH player encounters
//! this EventStream, it GETs the URL in the Event's text content.
//!
//! In SGAI mode the stitcher does NOT insert ad Periods. Instead it:
//! 1. Detects SCTE-35 ad breaks (reuses `detect_dash_ad_breaks`)
//! 2. Injects a callback EventStream in each Period containing an ad break
//! 3. Strips original SCTE-35 EventStreams to avoid double-signaling
//! 4. Rewrites content URLs (same as always)
//!
//! The callback URL points to the existing asset-list endpoint which returns
//! JSON: `{"ASSETS": [{"URI": "...", "DURATION": 15.0}]}`

use crate::dash::cue::{self, DashAdBreak};
use dash_mpd::{Event, EventStream, MPD};
use std::collections::HashMap;
use tracing::info;

/// DASH MPD Event callback scheme URI (ISO 23009-1)
const CALLBACK_SCHEME: &str = "urn:mpeg:dash:event:callback:2015";

/// Inject SGAI callback EventStreams for detected ad breaks.
///
/// For each ad break, adds an EventStream with the callback scheme to the
/// Period that contains the signal. The Event's text content (`content`) is
/// the asset-list URL for the player to GET when the event fires.
///
/// Ad breaks in the same Period are consolidated into a single EventStream
/// with multiple Events.
///
/// Uses `timescale=1` so `presentationTime` and `duration` are in seconds.
pub fn inject_dash_callbacks(
    mpd: &mut MPD,
    ad_breaks: &[DashAdBreak],
    session_id: &str,
    base_url: &str,
) {
    if ad_breaks.is_empty() {
        info!("No ad breaks detected, skipping DASH SGAI injection");
        return;
    }

    // Group ad breaks by period_index — one callback EventStream per Period
    let mut breaks_by_period: HashMap<usize, Vec<(usize, &DashAdBreak)>> = HashMap::new();
    for (break_idx, ad_break) in ad_breaks.iter().enumerate() {
        breaks_by_period
            .entry(ad_break.period_index)
            .or_default()
            .push((break_idx, ad_break));
    }

    for (period_idx, breaks) in &breaks_by_period {
        let Some(period) = mpd.periods.get_mut(*period_idx) else {
            continue;
        };

        let events: Vec<Event> = breaks
            .iter()
            .map(|(break_idx, ad_break)| {
                let callback_url = format!(
                    "{}/stitch/{}/asset-list/{}?dur={}",
                    base_url, session_id, break_idx, ad_break.duration as u64,
                );

                info!(
                    "DASH SGAI: injecting callback at Period #{}: duration={}s",
                    period_idx, ad_break.duration
                );

                Event {
                    id: Some(format!("ad-break-{}", break_idx)),
                    presentationTime: Some(ad_break.presentation_time as u64),
                    duration: Some(ad_break.duration as u64),
                    content: Some(callback_url),
                    ..Default::default()
                }
            })
            .collect();

        let callback_stream = EventStream {
            schemeIdUri: Some(CALLBACK_SCHEME.to_string()),
            timescale: Some(1),
            event: events,
            ..Default::default()
        };

        period.event_streams.push(callback_stream);
    }

    info!(
        "DASH SGAI: injected {} callback(s) across {} period(s)",
        ad_breaks.len(),
        breaks_by_period.len()
    );
}

/// Remove SCTE-35 EventStreams from all Periods to avoid double-signaling.
///
/// Retains any non-SCTE-35 EventStreams (including the callback EventStream
/// injected by `inject_dash_callbacks`).
pub fn strip_scte35_event_streams(mpd: &mut MPD) {
    for period in &mut mpd.periods {
        period.event_streams.retain(|es| {
            let scheme = es.schemeIdUri.as_deref().unwrap_or("");
            !cue::is_scte35_scheme(scheme)
        });
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dash::cue::DashSignalType;
    use dash_mpd::Period;

    fn make_scte35_event_stream() -> EventStream {
        EventStream {
            schemeIdUri: Some("urn:scte:scte35:2013:xml".to_string()),
            timescale: Some(1),
            event: vec![Event {
                id: Some("scte-1".to_string()),
                presentationTime: Some(15),
                duration: Some(10),
                ..Default::default()
            }],
            ..Default::default()
        }
    }

    fn make_non_scte35_event_stream() -> EventStream {
        EventStream {
            schemeIdUri: Some("urn:example:custom:2024".to_string()),
            timescale: Some(1),
            event: vec![],
            ..Default::default()
        }
    }

    fn make_ad_break(period_index: usize, presentation_time: f64, duration: f64) -> DashAdBreak {
        DashAdBreak {
            period_index,
            period_id: Some(format!("content-{}", period_index)),
            duration,
            presentation_time,
            signal_type: DashSignalType::SpliceInsert,
        }
    }

    fn make_mpd_with_periods(n: usize) -> MPD {
        let mut mpd = MPD::default();
        for i in 0..n {
            let mut period = Period {
                id: Some(format!("content-{}", i)),
                ..Default::default()
            };
            period.event_streams.push(make_scte35_event_stream());
            mpd.periods.push(period);
        }
        mpd
    }

    #[test]
    fn test_inject_single_callback() {
        let mut mpd = make_mpd_with_periods(2);
        let ad_breaks = vec![make_ad_break(0, 15.0, 10.0)];

        inject_dash_callbacks(&mut mpd, &ad_breaks, "test-session", "http://stitcher");

        // Period 0 should have the original SCTE-35 + new callback EventStream
        assert_eq!(mpd.periods[0].event_streams.len(), 2);
        let callback = &mpd.periods[0].event_streams[1];
        assert_eq!(callback.schemeIdUri.as_deref().unwrap(), CALLBACK_SCHEME);
        assert_eq!(callback.timescale, Some(1));
        assert_eq!(callback.event.len(), 1);
        assert_eq!(callback.event[0].id.as_deref().unwrap(), "ad-break-0");
        assert_eq!(callback.event[0].presentationTime, Some(15));
        assert_eq!(callback.event[0].duration, Some(10));

        // Period 1 should be unaffected
        assert_eq!(mpd.periods[1].event_streams.len(), 1);
    }

    #[test]
    fn test_inject_multiple_callbacks_different_periods() {
        let mut mpd = make_mpd_with_periods(3);
        let ad_breaks = vec![make_ad_break(0, 15.0, 10.0), make_ad_break(2, 20.0, 30.0)];

        inject_dash_callbacks(&mut mpd, &ad_breaks, "sess", "http://s");

        // Period 0: SCTE-35 + callback
        assert_eq!(mpd.periods[0].event_streams.len(), 2);
        // Period 1: only SCTE-35 (no ad break here)
        assert_eq!(mpd.periods[1].event_streams.len(), 1);
        // Period 2: SCTE-35 + callback
        assert_eq!(mpd.periods[2].event_streams.len(), 2);

        let cb2 = &mpd.periods[2].event_streams[1];
        assert_eq!(cb2.event[0].id.as_deref().unwrap(), "ad-break-1");
        assert_eq!(cb2.event[0].duration, Some(30));
    }

    #[test]
    fn test_inject_multiple_breaks_same_period() {
        let mut mpd = make_mpd_with_periods(1);
        let ad_breaks = vec![make_ad_break(0, 10.0, 15.0), make_ad_break(0, 40.0, 20.0)];

        inject_dash_callbacks(&mut mpd, &ad_breaks, "sess", "http://s");

        // One callback EventStream with 2 Events
        assert_eq!(mpd.periods[0].event_streams.len(), 2);
        let callback = &mpd.periods[0].event_streams[1];
        assert_eq!(callback.event.len(), 2);
    }

    #[test]
    fn test_strip_scte35_event_streams() {
        let mut mpd = MPD::default();
        let mut period = Period::default();
        period.event_streams.push(make_scte35_event_stream());
        period.event_streams.push(make_non_scte35_event_stream());
        mpd.periods.push(period);

        strip_scte35_event_streams(&mut mpd);

        // Only the non-SCTE-35 EventStream should remain
        assert_eq!(mpd.periods[0].event_streams.len(), 1);
        assert_eq!(
            mpd.periods[0].event_streams[0]
                .schemeIdUri
                .as_deref()
                .unwrap(),
            "urn:example:custom:2024"
        );
    }

    #[test]
    fn test_strip_preserves_callback_eventstream() {
        let mut mpd = make_mpd_with_periods(1);
        let ad_breaks = vec![make_ad_break(0, 15.0, 10.0)];

        // Inject callback, then strip SCTE-35
        inject_dash_callbacks(&mut mpd, &ad_breaks, "sess", "http://s");
        strip_scte35_event_streams(&mut mpd);

        // Only the callback EventStream should remain
        assert_eq!(mpd.periods[0].event_streams.len(), 1);
        assert_eq!(
            mpd.periods[0].event_streams[0]
                .schemeIdUri
                .as_deref()
                .unwrap(),
            CALLBACK_SCHEME
        );
    }

    #[test]
    fn test_no_ad_periods_inserted() {
        let mut mpd = make_mpd_with_periods(2);
        let original_period_count = mpd.periods.len();
        let ad_breaks = vec![make_ad_break(0, 15.0, 10.0)];

        inject_dash_callbacks(&mut mpd, &ad_breaks, "sess", "http://s");

        // Period count must be unchanged — SGAI never inserts new Periods
        assert_eq!(mpd.periods.len(), original_period_count);
    }

    #[test]
    fn test_callback_url_format() {
        let mut mpd = make_mpd_with_periods(1);
        let ad_breaks = vec![make_ad_break(0, 15.0, 30.0)];

        inject_dash_callbacks(
            &mut mpd,
            &ad_breaks,
            "my-session",
            "https://stitcher.example.com",
        );

        let callback = &mpd.periods[0].event_streams[1];
        let url = callback.event[0].content.as_deref().unwrap();
        assert_eq!(
            url,
            "https://stitcher.example.com/stitch/my-session/asset-list/0?dur=30"
        );
    }

    #[test]
    fn test_empty_ad_breaks_noop() {
        let mut mpd = make_mpd_with_periods(1);
        let ad_breaks: Vec<DashAdBreak> = vec![];

        inject_dash_callbacks(&mut mpd, &ad_breaks, "sess", "http://s");

        // Only the original SCTE-35 EventStream
        assert_eq!(mpd.periods[0].event_streams.len(), 1);
    }
}
