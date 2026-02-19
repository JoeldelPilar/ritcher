use crate::ad::vast::TrackingEvent;
use crate::metrics;
use reqwest::Client;
use tracing::{info, warn};

/// Determine which tracking events should fire for this segment
///
/// Maps segment index to quartile progress and returns matching tracking events.
/// Quartile calculation: segment_index / (total_segments - 1)
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

    let mut events = Vec::new();

    for event in tracking_events {
        let should_fire = match event.event.as_str() {
            "start" => segment_index == 0,
            "firstQuartile" => (0.25..0.50).contains(&progress),
            "midpoint" => (0.50..0.75).contains(&progress),
            "thirdQuartile" => (0.75..1.0).contains(&progress),
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
/// # Arguments
/// * `client` - HTTP client for beacon request
/// * `url` - Tracking beacon URL
/// * `event_name` - Name of the event being tracked (for logging/metrics)
pub fn fire_beacon(client: Client, url: String, event_name: String) {
    tokio::spawn(async move {
        match client.get(&url).send().await {
            Ok(resp) => {
                info!(
                    "Tracking beacon fired: {} -> {} ({})",
                    event_name,
                    url,
                    resp.status()
                );
                metrics::record_tracking_event(&event_name, "success");
            }
            Err(e) => {
                warn!("Tracking beacon failed: {} -> {} ({})", event_name, url, e);
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
        let result = events_for_segment(1, 4, &events);
        assert!(result.iter().any(|e| e.event == "firstQuartile"));
    }

    #[test]
    fn test_midpoint() {
        let events = make_events();
        let result = events_for_segment(2, 4, &events);
        assert!(result.iter().any(|e| e.event == "midpoint"));
    }

    #[test]
    fn test_third_quartile() {
        let events = make_events();
        // With 5 segments: 3/4 = 75% progress → thirdQuartile
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
}
