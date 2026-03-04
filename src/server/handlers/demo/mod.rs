mod dash;
mod hls;

use serde::Deserialize;

// Re-export all handler functions
pub use dash::serve_demo_manifest;
pub use hls::{serve_demo_ll_hls_playlist, serve_demo_playlist};

/// Base URL for Mux Big Buck Bunny test stream segments (HLS, MPEG-TS)
const MUX_BASE: &str = "https://test-streams.mux.dev/x36xhzz/url_0";
/// Mux segment filename
const MUX_SEGMENT: &str = "193039199_mp4_h264_aac_hd_7.ts";
/// First Mux segment index
const MUX_START_INDEX: u32 = 462;
/// Duration of each HLS segment in seconds
const SEGMENT_DURATION: f32 = 10.0;
/// Duration of each ad break in seconds (matches DemoAdProvider: 10 x 1s segments)
const BREAK_DURATION: u32 = 10;
/// Number of placeholder content segments per ad break (10s / 10s = 1)
const BREAK_SEGMENTS: u32 = 1;

/// Base URL for DASH-IF Big Buck Bunny fMP4 segments (DASH, ISO BMFF)
const DASH_BASE: &str = "https://dash.akamaized.net/akamai/bbb_30fps";
/// DASH video representation ID (640x360 @ 800 kbps)
const DASH_VIDEO_REP: &str = "bbb_30fps_640x360_800k";
/// DASH audio representation ID (AAC @ 64 kbps)
const DASH_AUDIO_REP: &str = "bbb_a64k";
/// Duration of each DASH segment in seconds
const DASH_SEGMENT_DURATION: f32 = 4.0;
/// First DASH segment number
const DASH_START_NUMBER: u32 = 1;

/// Query parameters for configurable demo endpoints
#[derive(Debug, Deserialize)]
pub struct DemoParams {
    /// Number of ad breaks (1-5, default: 1)
    breaks: Option<u8>,
    /// Seconds of content between ad breaks (10, 20, 30; default: 10)
    interval: Option<u8>,
}

impl DemoParams {
    /// Validated number of breaks, clamped to 1..=5
    fn num_breaks(&self) -> u8 {
        self.breaks.unwrap_or(1).clamp(1, 5)
    }

    /// Validated interval in seconds, snapped to nearest allowed value
    fn interval_secs(&self) -> u8 {
        match self.interval.unwrap_or(10) {
            0..=14 => 10,
            15..=24 => 20,
            _ => 30,
        }
    }
}

/// Build a Mux segment URL for the given index
fn mux_segment_url(index: u32) -> String {
    format!("{}/url_{}/{}", MUX_BASE, index, MUX_SEGMENT)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_demo_params_defaults() {
        let params = DemoParams {
            breaks: None,
            interval: None,
        };
        assert_eq!(params.num_breaks(), 1);
        assert_eq!(params.interval_secs(), 10);
    }

    #[test]
    fn test_demo_params_clamping() {
        // Breaks clamped to 1..=5
        let p = DemoParams {
            breaks: Some(0),
            interval: None,
        };
        assert_eq!(p.num_breaks(), 1);

        let p = DemoParams {
            breaks: Some(10),
            interval: None,
        };
        assert_eq!(p.num_breaks(), 5);

        // Interval snapping
        let p = DemoParams {
            breaks: None,
            interval: Some(5),
        };
        assert_eq!(p.interval_secs(), 10);

        let p = DemoParams {
            breaks: None,
            interval: Some(14),
        };
        assert_eq!(p.interval_secs(), 10);

        let p = DemoParams {
            breaks: None,
            interval: Some(22),
        };
        assert_eq!(p.interval_secs(), 20);

        let p = DemoParams {
            breaks: None,
            interval: Some(35),
        };
        assert_eq!(p.interval_secs(), 30);
    }
}
