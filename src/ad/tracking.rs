use crate::ad::vast::TrackingEvent;
use crate::metrics;
use reqwest::Client;
use std::time::Duration;
use tracing::{debug, info, warn};

/// Determine which tracking events should fire for this segment
///
/// Uses "threshold crossing" logic: an event fires on the first segment
/// whose progress crosses the quartile boundary. This ensures all VAST
/// quartile events fire even for short ads (2–3 segments).
///
/// Quartile thresholds: start=0%, firstQuartile=25%, midpoint=50%,
/// thirdQuartile=75%, complete=last segment.
///
/// # Arguments
/// * `segment_index` - Index of the segment being served (0-based)
/// * `total_segments` - Total number of segments in the ad
/// * `tracking_events` - All available tracking events from VAST
///
/// # Returns
/// Vector of tracking events that should fire for this segment
pub fn events_for_segment(
    segment_index: usize,
    total_segments: usize,
    tracking_events: &[TrackingEvent],
) -> Vec<&TrackingEvent> {
    if total_segments == 0 {
        return Vec::new();
    }

    let progress = if total_segments == 1 {
        1.0 // Single segment = 100%
    } else {
        segment_index as f64 / (total_segments - 1) as f64
    };

    // Previous segment's progress (for detecting threshold crossings)
    let prev_progress = if segment_index == 0 || total_segments == 1 {
        -1.0 // Sentinel: no previous segment
    } else {
        (segment_index - 1) as f64 / (total_segments - 1) as f64
    };

    let mut events = Vec::new();

    for event in tracking_events {
        let should_fire = match event.event.as_str() {
            "start" => segment_index == 0,
            "firstQuartile" => progress >= 0.25 && prev_progress < 0.25,
            "midpoint" => progress >= 0.50 && prev_progress < 0.50,
            "thirdQuartile" => progress >= 0.75 && prev_progress < 0.75,
            "complete" => segment_index == total_segments - 1,
            _ => false, // Ignore unknown events
        };

        if should_fire {
            events.push(event);
        }
    }

    events
}

/// Fire a tracking beacon (fire-and-forget)
///
/// Spawns a background task. Does not block the caller.
/// No retries -- best effort as per VAST spec.
///
/// **Concurrency note:** Beacons are spawned via `tokio::spawn` without an
/// explicit concurrency limit. The `reqwest::Client` connection pool provides
/// natural backpressure (default: ~100 idle connections per host). Under peak
/// load with many concurrent ad breaks, this is sufficient. A `Semaphore`-based
/// limit can be added if telemetry shows connection exhaustion.
///
/// # Arguments
/// * `client` - HTTP client for beacon request
/// * `url` - Tracking beacon URL
/// * `event_name` - Name of the event being tracked (for logging/metrics)
pub fn fire_beacon(client: Client, url: String, event_name: String) {
    tokio::spawn(async move {
        match client
            .get(&url)
            .timeout(Duration::from_secs(2))
            .send()
            .await
        {
            Ok(resp) => {
                let status = resp.status();
                debug!("Tracking beacon: {} -> {} ({})", event_name, url, status);
                info!("Tracking beacon: {} ({})", event_name, status);
                metrics::record_tracking_event(&event_name, "success");
            }
            Err(e) => {
                debug!("Tracking beacon URL: {}", url);
                warn!("Tracking beacon failed: {} ({})", event_name, e);
                metrics::record_tracking_event(&event_name, "error");
            }
        }
    });
}

/// Fire impression beacons for an ad
///
/// Impressions are fired when the first segment of an ad is served.
///
/// # Arguments
/// * `client` - HTTP client
/// * `impression_urls` - List of impression tracking URLs from VAST
pub fn fire_impressions(client: Client, impression_urls: &[String]) {
    for url in impression_urls {
        fire_beacon(client.clone(), url.clone(), "impression".to_string());
    }
}

/// Fire error beacon
///
/// Called when VAST fetch or ad segment fetch fails.
///
/// # Arguments
/// * `client` - HTTP client
/// * `error_url` - Error tracking URL from VAST
pub fn fire_error(client: Client, error_url: &str) {
    fire_beacon(client, error_url.to_string(), "error".to_string());
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ad::vast::TrackingEvent;

    fn make_events() -> Vec<TrackingEvent> {
        vec![
            TrackingEvent {
                event: "start".into(),
                url: "http://t/start".into(),
            },
            TrackingEvent {
                event: "firstQuartile".into(),
                url: "http://t/fq".into(),
            },
            TrackingEvent {
                event: "midpoint".into(),
                url: "http://t/mid".into(),
            },
            TrackingEvent {
                event: "thirdQuartile".into(),
                url: "http://t/tq".into(),
            },
            TrackingEvent {
                event: "complete".into(),
                url: "http://t/complete".into(),
            },
        ]
    }

    #[test]
    fn test_start_event_on_first_segment() {
        let events = make_events();
        let result = events_for_segment(0, 4, &events);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].event, "start");
    }

    #[test]
    fn test_first_quartile() {
        let events = make_events();
        // 4 segments: seg 1 progress = 1/3 ≈ 0.333, prev = 0.0 → crosses 0.25
        let result = events_for_segment(1, 4, &events);
        assert!(result.iter().any(|e| e.event == "firstQuartile"));
    }

    #[test]
    fn test_midpoint() {
        let events = make_events();
        // 4 segments: seg 2 progress = 2/3 ≈ 0.667, prev = 1/3 ≈ 0.333 → crosses 0.50
        let result = events_for_segment(2, 4, &events);
        assert!(result.iter().any(|e| e.event == "midpoint"));
    }

    #[test]
    fn test_third_quartile() {
        let events = make_events();
        // 5 segments: seg 3 progress = 3/4 = 0.75, prev = 2/4 = 0.50 → crosses 0.75
        let result = events_for_segment(3, 5, &events);
        assert!(result.iter().any(|e| e.event == "thirdQuartile"));
    }

    #[test]
    fn test_complete_on_last_segment() {
        let events = make_events();
        let result = events_for_segment(3, 4, &events);
        assert!(result.iter().any(|e| e.event == "complete"));
    }

    #[test]
    fn test_single_segment_fires_start_and_complete() {
        let events = make_events();
        let result = events_for_segment(0, 1, &events);
        assert!(result.iter().any(|e| e.event == "start"));
        assert!(result.iter().any(|e| e.event == "complete"));
    }

    #[test]
    fn test_no_events_for_empty_tracking() {
        let result = events_for_segment(0, 4, &[]);
        assert!(result.is_empty());
    }

    #[test]
    fn test_zero_total_segments() {
        let events = make_events();
        let result = events_for_segment(0, 0, &events);
        assert!(result.is_empty());
    }

    #[test]
    fn test_two_segments_fires_all_quartiles() {
        let events = make_events();
        // Segment 0: start only
        let seg0 = events_for_segment(0, 2, &events);
        assert!(seg0.iter().any(|e| e.event == "start"));
        assert_eq!(seg0.len(), 1);

        // Segment 1: crosses 0.25, 0.50, 0.75 → all quartiles + complete
        let seg1 = events_for_segment(1, 2, &events);
        assert!(seg1.iter().any(|e| e.event == "firstQuartile"));
        assert!(seg1.iter().any(|e| e.event == "midpoint"));
        assert!(seg1.iter().any(|e| e.event == "thirdQuartile"));
        assert!(seg1.iter().any(|e| e.event == "complete"));
    }

    #[test]
    fn test_three_segments_fires_all_quartiles() {
        let events = make_events();
        // Segment 0 (progress=0.0): start
        let seg0 = events_for_segment(0, 3, &events);
        assert!(seg0.iter().any(|e| e.event == "start"));

        // Segment 1 (progress=0.5, prev=0.0): crosses 0.25 and 0.50
        let seg1 = events_for_segment(1, 3, &events);
        assert!(seg1.iter().any(|e| e.event == "firstQuartile"));
        assert!(seg1.iter().any(|e| e.event == "midpoint"));

        // Segment 2 (progress=1.0, prev=0.5): crosses 0.75 + complete
        let seg2 = events_for_segment(2, 3, &events);
        assert!(seg2.iter().any(|e| e.event == "thirdQuartile"));
        assert!(seg2.iter().any(|e| e.event == "complete"));
    }

    #[test]
    fn test_single_segment_fires_all_events() {
        let events = make_events();
        // Single segment: progress=1.0, prev=-1.0 → crosses all thresholds
        let result = events_for_segment(0, 1, &events);
        assert!(result.iter().any(|e| e.event == "start"));
        assert!(result.iter().any(|e| e.event == "firstQuartile"));
        assert!(result.iter().any(|e| e.event == "midpoint"));
        assert!(result.iter().any(|e| e.event == "thirdQuartile"));
        assert!(result.iter().any(|e| e.event == "complete"));
        assert_eq!(result.len(), 5);
    }
}
