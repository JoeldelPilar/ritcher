//! HLS Interstitials asset-list endpoint
//!
//! Called by HLS players (hls.js ≥1.6, AVPlayer) when they encounter an
//! `EXT-X-DATERANGE` tag with `CLASS="com.apple.hls.interstitial"` and
//! `X-ASSET-LIST` pointing to this endpoint.
//!
//! Returns a JSON asset list conforming to RFC 8216bis §6.3:
//! ```json
//! {"ASSETS": [{"URI": "https://ad-cdn.example.com/ad.m3u8", "DURATION": 30.0}]}
//! ```
//!
//! When VAST contains `<AdVerifications>` with OMID verification nodes, the
//! response includes an `X-VERIFICATIONS` array so SGAI clients can load the
//! third-party measurement scripts:
//! ```json
//! {"ASSETS": [...], "X-VERIFICATIONS": [{"vendor": "...", "resource": "...", ...}]}
//! ```

use crate::ad::vast::Verification;
use crate::{error::Result, metrics, server::state::AppState};
use axum::{
    Json,
    extract::{Path, Query, State},
    response::{IntoResponse, Response},
};
use serde::Serialize;
use std::collections::HashMap;
use std::time::Instant;
use tracing::info;

/// HLS Interstitials asset-list response
#[derive(Serialize)]
struct AssetList {
    #[serde(rename = "ASSETS")]
    assets: Vec<Asset>,
    /// OMID verification resources — only present when VAST contained `<AdVerifications>`
    #[serde(rename = "X-VERIFICATIONS", skip_serializing_if = "Vec::is_empty")]
    verifications: Vec<VerificationOutput>,
}

/// Single asset entry in the asset-list
#[derive(Serialize)]
struct Asset {
    #[serde(rename = "URI")]
    uri: String,
    #[serde(rename = "DURATION")]
    duration: f64,
}

/// Serializable OMID verification resource for the asset-list JSON
#[derive(Debug, Clone, Serialize, PartialEq)]
struct VerificationOutput {
    #[serde(skip_serializing_if = "Option::is_none")]
    vendor: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    resource: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    parameters: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    api_framework: Option<String>,
}

impl From<&Verification> for VerificationOutput {
    fn from(v: &Verification) -> Self {
        Self {
            vendor: v.vendor.clone(),
            resource: v.javascript_resource_url.clone(),
            parameters: v.parameters.clone(),
            api_framework: v.api_framework.clone(),
        }
    }
}

/// Serve HLS Interstitials asset-list JSON
///
/// Called by the player for each ad break it encounters. Returns the list of
/// ad creatives (URI + duration) the player should fetch and play inline.
///
/// Query params:
/// - `dur` — requested ad break duration in seconds (default: 30.0)
pub async fn serve_asset_list(
    Path((session_id, break_id)): Path<(String, String)>,
    Query(params): Query<HashMap<String, String>>,
    State(state): State<AppState>,
) -> Result<Response> {
    let start = Instant::now();
    info!(
        "Serving asset-list for session: {} break: {}",
        session_id, break_id
    );

    let duration: f32 = params
        .get("dur")
        .and_then(|d| d.parse().ok())
        .unwrap_or(30.0);

    let creatives = state
        .ad_provider
        .get_ad_creatives(duration, &session_id)
        .await;

    // Collect all unique verifications across all creatives.
    // In a typical VAST response all creatives from the same InLine share the
    // same verification nodes, but wrapper chains can add more.  We deduplicate
    // by (vendor, resource) to avoid sending the same script twice.
    let mut all_verifications: Vec<VerificationOutput> = Vec::new();
    for creative in &creatives {
        for v in &creative.verifications {
            let output = VerificationOutput::from(v);
            if !all_verifications.contains(&output) {
                all_verifications.push(output);
            }
        }
    }

    let assets: Vec<Asset> = creatives
        .into_iter()
        .map(|c| Asset {
            uri: c.uri,
            duration: c.duration,
        })
        .collect();

    info!(
        "Asset-list: {} creative(s), {} verification(s) for session {} (duration {}s)",
        assets.len(),
        all_verifications.len(),
        session_id,
        duration
    );

    metrics::record_asset_list_request(200);
    metrics::record_duration("asset_list", start);

    Ok(Json(AssetList {
        assets,
        verifications: all_verifications,
    })
    .into_response())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ad::vast::{Verification, VerificationTrackingEvent};

    #[test]
    fn test_verification_output_from_verification() {
        let v = Verification {
            vendor: Some("doubleverify.com-omid".to_string()),
            javascript_resource_url: Some("https://cdn.dv.com/dvtp_src.js".to_string()),
            api_framework: Some("omid".to_string()),
            parameters: Some("ctx=123".to_string()),
            tracking_events: vec![VerificationTrackingEvent {
                event: "verificationNotExecuted".to_string(),
                uri: "https://verify.example.com/failed".to_string(),
            }],
        };

        let output = VerificationOutput::from(&v);
        assert_eq!(output.vendor.as_deref(), Some("doubleverify.com-omid"));
        assert_eq!(
            output.resource.as_deref(),
            Some("https://cdn.dv.com/dvtp_src.js")
        );
        assert_eq!(output.parameters.as_deref(), Some("ctx=123"));
        assert_eq!(output.api_framework.as_deref(), Some("omid"));
    }

    #[test]
    fn test_verification_output_from_minimal_verification() {
        let v = Verification {
            vendor: None,
            javascript_resource_url: Some("https://example.com/verify.js".to_string()),
            api_framework: None,
            parameters: None,
            tracking_events: vec![],
        };

        let output = VerificationOutput::from(&v);
        assert!(output.vendor.is_none());
        assert_eq!(
            output.resource.as_deref(),
            Some("https://example.com/verify.js")
        );
        assert!(output.api_framework.is_none());
        assert!(output.parameters.is_none());
    }

    #[test]
    fn test_asset_list_json_without_verifications() {
        let list = AssetList {
            assets: vec![Asset {
                uri: "https://ad.example.com/ad.m3u8".to_string(),
                duration: 30.0,
            }],
            verifications: vec![],
        };

        let json = serde_json::to_string(&list).unwrap();
        assert!(json.contains("\"ASSETS\""));
        assert!(
            !json.contains("X-VERIFICATIONS"),
            "Empty verifications should not appear in JSON (skip_serializing_if)"
        );
    }

    #[test]
    fn test_asset_list_json_with_verifications() {
        let list = AssetList {
            assets: vec![Asset {
                uri: "https://ad.example.com/ad.m3u8".to_string(),
                duration: 15.0,
            }],
            verifications: vec![VerificationOutput {
                vendor: Some("doubleverify.com-omid".to_string()),
                resource: Some("https://cdn.dv.com/dvtp_src.js".to_string()),
                parameters: Some("ctx=123".to_string()),
                api_framework: Some("omid".to_string()),
            }],
        };

        let json = serde_json::to_string(&list).unwrap();
        assert!(json.contains("\"X-VERIFICATIONS\""));
        assert!(json.contains("doubleverify.com-omid"));
        assert!(json.contains("https://cdn.dv.com/dvtp_src.js"));
        assert!(json.contains("ctx=123"));
        assert!(json.contains("omid"));
    }

    #[test]
    fn test_asset_list_json_verification_skips_none_fields() {
        let list = AssetList {
            assets: vec![],
            verifications: vec![VerificationOutput {
                vendor: None,
                resource: Some("https://example.com/verify.js".to_string()),
                parameters: None,
                api_framework: None,
            }],
        };

        let json = serde_json::to_string(&list).unwrap();
        // Fields with None should not appear in the JSON output
        assert!(!json.contains("\"vendor\""));
        assert!(!json.contains("\"parameters\""));
        assert!(!json.contains("\"api_framework\""));
        assert!(json.contains("\"resource\""));
    }

    #[test]
    fn test_verification_dedup_by_equality() {
        let v1 = VerificationOutput {
            vendor: Some("dv".to_string()),
            resource: Some("https://cdn.dv.com/script.js".to_string()),
            parameters: None,
            api_framework: Some("omid".to_string()),
        };
        let v2 = v1.clone();
        let v3 = VerificationOutput {
            vendor: Some("ias".to_string()),
            resource: Some("https://cdn.ias.com/script.js".to_string()),
            parameters: None,
            api_framework: Some("omid".to_string()),
        };

        let mut all: Vec<VerificationOutput> = Vec::new();
        for v in &[v1, v2, v3] {
            if !all.contains(v) {
                all.push(v.clone());
            }
        }

        assert_eq!(
            all.len(),
            2,
            "Duplicate verifications should be deduplicated"
        );
        assert_eq!(all[0].vendor.as_deref(), Some("dv"));
        assert_eq!(all[1].vendor.as_deref(), Some("ias"));
    }
}
