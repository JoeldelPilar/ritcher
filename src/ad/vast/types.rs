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
    /// OMID verification resources from `<AdVerifications>`
    pub verifications: Vec<Verification>,
}

/// Wrapper ad that references another VAST tag
#[derive(Debug, Clone)]
pub struct WrapperAd {
    pub ad_tag_uri: String,
    pub impression_urls: Vec<String>,
    pub tracking_events: Vec<TrackingEvent>,
    /// OMID verification resources from `<AdVerifications>` in the wrapper
    pub verifications: Vec<Verification>,
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

/// A single OM SDK verification resource from `<AdVerifications>`.
///
/// OMID (Open Measurement Interface Definition) verification nodes allow
/// third-party viewability/measurement scripts to be passed through to the
/// player. In SGAI mode these are serialized in the asset-list JSON so the
/// client-side player can load the verification JS.
#[derive(Debug, Clone, PartialEq)]
pub struct Verification {
    /// Vendor key, e.g. "doubleverify.com-omid"
    pub vendor: Option<String>,
    /// URL to the verification JavaScript resource
    pub javascript_resource_url: Option<String>,
    /// API framework, expected value: "omid"
    pub api_framework: Option<String>,
    /// Optional `<VerificationParameters>` CDATA content (opaque string)
    pub parameters: Option<String>,
    /// Optional tracking events within this `<Verification>` node
    pub tracking_events: Vec<VerificationTrackingEvent>,
}

/// Tracking event within a `<Verification>` node (e.g. `verificationNotExecuted`).
#[derive(Debug, Clone, PartialEq)]
pub struct VerificationTrackingEvent {
    pub event: String,
    pub uri: String,
}
