use std::env;

/// Ad provider selection
#[derive(Clone, Debug, PartialEq)]
pub enum AdProviderType {
    /// Static ad provider using pre-configured segments (default for dev)
    Static,
    /// VAST-based ad provider fetching from an ad server
    Vast,
}

/// Application configuration loaded from environment variables
#[derive(Clone, Debug)]
pub struct Config {
    pub port: u16,
    pub base_url: String,
    pub origin_url: String,
    pub is_dev: bool,
    /// Ad provider type selection
    pub ad_provider_type: AdProviderType,
    /// Static ad source URL (used when ad_provider_type = Static)
    pub ad_source_url: String,
    /// Static ad segment duration (used when ad_provider_type = Static)
    pub ad_segment_duration: f32,
    /// VAST endpoint URL (used when ad_provider_type = Vast)
    pub vast_endpoint: Option<String>,
    /// Slate URL for fallback content when no ads are available
    pub slate_url: Option<String>,
    /// Slate segment duration in seconds (default: 1.0)
    pub slate_segment_duration: f32,
}

impl Config {
    /// Load configuration from environment variables
    /// In DEV mode, provides sensible defaults. In PROD mode, all vars are required.
    pub fn from_env() -> Result<Self, Box<dyn std::error::Error>> {
        // Check if running in dev mode
        let is_dev = env::var("DEV_MODE")
            .unwrap_or_else(|_| "false".to_string())
            .parse()
            .unwrap_or(false);

        // Port: required in prod, defaults to 3000 in dev
        let port = if is_dev {
            env::var("PORT")
                .unwrap_or_else(|_| "3000".to_string())
                .parse()?
        } else {
            env::var("PORT")
                .map_err(|_| "PORT is required in production")?
                .parse()?
        };

        // Base URL: required in prod, defaults to localhost in dev
        let base_url = if is_dev {
            env::var("BASE_URL").unwrap_or_else(|_| "http://localhost:3000".to_string())
        } else {
            env::var("BASE_URL").map_err(|_| "BASE_URL is required in production")?
        };

        // Origin URL: required in prod, defaults to example.com in dev
        let origin_url = if is_dev {
            env::var("ORIGIN_URL").unwrap_or_else(|_| "https://example.com".to_string())
        } else {
            env::var("ORIGIN_URL").map_err(|_| "ORIGIN_URL is required in production")?
        };

        // VAST endpoint URL (optional)
        let vast_endpoint = env::var("VAST_ENDPOINT").ok();

        // Ad provider type: auto-detect from VAST_ENDPOINT or explicit AD_PROVIDER_TYPE
        let ad_provider_type = match env::var("AD_PROVIDER_TYPE")
            .unwrap_or_else(|_| "auto".to_string())
            .to_lowercase()
            .as_str()
        {
            "vast" => AdProviderType::Vast,
            "static" => AdProviderType::Static,
            _ => {
                // Auto-detect: use VAST if endpoint is configured, otherwise static
                if vast_endpoint.is_some() {
                    AdProviderType::Vast
                } else {
                    AdProviderType::Static
                }
            }
        };

        // Static ad source URL: defaults to test ad stream
        let ad_source_url = env::var("AD_SOURCE_URL")
            .unwrap_or_else(|_| "https://hls.src.tedm.io/content/ts_h264_480p_1s".to_string());

        // Static ad segment duration: defaults to 1 second
        let ad_segment_duration = env::var("AD_SEGMENT_DURATION")
            .unwrap_or_else(|_| "1.0".to_string())
            .parse()
            .unwrap_or(1.0);

        // Slate URL: optional fallback content for empty ad breaks
        let slate_url = env::var("SLATE_URL").ok();

        // Slate segment duration: defaults to 1 second
        let slate_segment_duration = env::var("SLATE_SEGMENT_DURATION")
            .unwrap_or_else(|_| "1.0".to_string())
            .parse()
            .unwrap_or(1.0);

        Ok(Config {
            port,
            base_url,
            origin_url,
            is_dev,
            ad_provider_type,
            ad_source_url,
            ad_segment_duration,
            vast_endpoint,
            slate_url,
            slate_segment_duration,
        })
    }
}
