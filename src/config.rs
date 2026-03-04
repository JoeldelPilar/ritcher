use std::env;
use tracing::warn;

/// HLS stitching mode
#[derive(Clone, Debug, PartialEq)]
pub enum StitchingMode {
    /// Server-Side Ad Insertion: stitcher replaces content segments with ad segments
    Ssai,
    /// Server-Guided Ad Insertion: stitcher injects EXT-X-DATERANGE interstitial markers,
    /// player fetches and plays ads client-side (HLS Interstitials spec)
    Sgai,
}

/// Session store type selection
#[derive(Clone, Debug, PartialEq)]
pub enum SessionStoreType {
    Memory,
    Valkey,
}

/// Ad provider selection
#[derive(Clone, Debug, PartialEq)]
pub enum AdProviderType {
    /// Static ad provider using pre-configured segments (default for dev)
    Static,
    /// VAST-based ad provider fetching from an ad server
    Vast,
    /// Demo ad provider with 5 visually distinct creatives per break
    Demo,
}

/// Application configuration loaded from environment variables.
///
/// In `DEV_MODE=true`, most fields have sensible defaults. In production,
/// `PORT`, `BASE_URL`, and `ORIGIN_URL` are required.
#[derive(Clone, Debug)]
pub struct Config {
    /// TCP port the HTTP server binds to (`PORT`, default: 3000 in dev)
    pub port: u16,
    /// Public URL of this stitcher instance (`BASE_URL`)
    pub base_url: String,
    /// Default origin playlist/manifest URL (`ORIGIN_URL`)
    pub origin_url: String,
    /// Whether development mode is active (`DEV_MODE`)
    pub is_dev: bool,
    /// HLS stitching mode: ssai (default) or sgai (`STITCHING_MODE`)
    pub stitching_mode: StitchingMode,
    /// Ad provider type selection (`AD_PROVIDER_TYPE`: auto, vast, static, demo)
    pub ad_provider_type: AdProviderType,
    /// Static ad source URL (`AD_SOURCE_URL`, used when ad_provider_type = Static)
    pub ad_source_url: String,
    /// Static ad segment duration in seconds (`AD_SEGMENT_DURATION`, default: 1.0)
    pub ad_segment_duration: f32,
    /// VAST ad server endpoint URL (`VAST_ENDPOINT`)
    pub vast_endpoint: Option<String>,
    /// Slate URL for fallback content when no ads are available (`SLATE_URL`)
    pub slate_url: Option<String>,
    /// Slate segment duration in seconds (`SLATE_SEGMENT_DURATION`, default: 1.0)
    pub slate_segment_duration: f32,
    /// Session store backend (`SESSION_STORE`: memory or valkey)
    pub session_store: SessionStoreType,
    /// Valkey/Redis connection URL (`VALKEY_URL`)
    pub valkey_url: Option<String>,
    /// Session TTL in seconds (`SESSION_TTL_SECS`, default: 300)
    pub session_ttl_secs: u64,
    /// Max requests per minute per IP, 0 = disabled (`RATE_LIMIT_RPM`)
    pub rate_limit_rpm: u32,
    /// Base URL for demo ad creatives (`DEMO_AD_BASE_URL`)
    pub demo_ad_base_url: Option<String>,
    /// Origin HTTP request timeout in seconds (`ORIGIN_TIMEOUT_SECS`, default: 30)
    pub origin_timeout_secs: u64,
    /// Manifest cache TTL in milliseconds (`MANIFEST_CACHE_TTL_MS`, default: 2000)
    pub manifest_cache_ttl_ms: u64,
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

        // Stitching mode: ssai (default) or sgai
        let stitching_mode = match env::var("STITCHING_MODE")
            .unwrap_or_else(|_| "ssai".to_string())
            .to_lowercase()
            .as_str()
        {
            "sgai" => StitchingMode::Sgai,
            _ => StitchingMode::Ssai,
        };

        // VAST endpoint URL (optional)
        let vast_endpoint = env::var("VAST_ENDPOINT").ok();

        // Demo ad base URL (for DemoAdProvider creative sources)
        let demo_ad_base_url = env::var("DEMO_AD_BASE_URL").ok();

        // Ad provider type: auto-detect from VAST_ENDPOINT or explicit AD_PROVIDER_TYPE
        let ad_provider_type_raw = env::var("AD_PROVIDER_TYPE")
            .unwrap_or_else(|_| "auto".to_string())
            .to_lowercase();
        let ad_provider_type = match ad_provider_type_raw.as_str() {
            "vast" => AdProviderType::Vast,
            "static" => AdProviderType::Static,
            "demo" => AdProviderType::Demo,
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

        let session_ttl_secs: u64 = env::var("SESSION_TTL_SECS")
            .unwrap_or_else(|_| "300".to_string())
            .parse()
            .unwrap_or(300);
        let session_store = match env::var("SESSION_STORE")
            .unwrap_or_else(|_| "memory".to_string())
            .to_lowercase()
            .as_str()
        {
            "valkey" | "redis" => SessionStoreType::Valkey,
            _ => SessionStoreType::Memory,
        };
        let valkey_url = env::var("VALKEY_URL").ok();

        let rate_limit_rpm: u32 = env::var("RATE_LIMIT_RPM")
            .unwrap_or_else(|_| "0".to_string())
            .parse()
            .unwrap_or(0);

        let origin_timeout_secs: u64 = env::var("ORIGIN_TIMEOUT_SECS")
            .unwrap_or_else(|_| "30".to_string())
            .parse()
            .unwrap_or(30);

        let manifest_cache_ttl_ms: u64 = env::var("MANIFEST_CACHE_TTL_MS")
            .unwrap_or_else(|_| "2000".to_string())
            .parse()
            .unwrap_or(2000);

        // Emit warnings for important silent fallbacks in production mode
        if !is_dev {
            if rate_limit_rpm == 0 {
                warn!("Rate limiting is disabled (RATE_LIMIT_RPM is 0 or unset)");
            }

            if vast_endpoint.is_none() && matches!(ad_provider_type_raw.as_str(), "auto" | "vast") {
                warn!("No VAST endpoint configured, falling back to static ads");
            }
        }

        Ok(Config {
            port,
            base_url,
            origin_url,
            is_dev,
            stitching_mode,
            ad_provider_type,
            ad_source_url,
            ad_segment_duration,
            vast_endpoint,
            slate_url,
            slate_segment_duration,
            session_store,
            valkey_url,
            session_ttl_secs,
            rate_limit_rpm,
            demo_ad_base_url,
            origin_timeout_secs,
            manifest_cache_ttl_ms,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// Serialize all env-var tests to prevent races between parallel test threads.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    /// Set env vars, run `f`, then restore original state.
    ///
    /// `set` — vars to set; `unset` — vars to remove before running `f`.
    fn with_env(set: &[(&str, &str)], unset: &[&str], f: impl FnOnce()) {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());

        // Save state for all touched vars
        let save_set: Vec<(&str, Option<String>)> = set
            .iter()
            .map(|(k, _)| (*k, std::env::var(k).ok()))
            .collect();
        let save_unset: Vec<(&str, Option<String>)> =
            unset.iter().map(|k| (*k, std::env::var(k).ok())).collect();

        for (k, v) in set {
            // SAFETY: serialized by ENV_LOCK — no other thread modifies env vars concurrently.
            unsafe { std::env::set_var(k, v) };
        }
        for k in unset {
            unsafe { std::env::remove_var(k) };
        }

        f();

        // Restore
        for (k, old) in save_set.into_iter().chain(save_unset) {
            match old {
                Some(v) => unsafe { std::env::set_var(k, v) },
                None => unsafe { std::env::remove_var(k) },
            }
        }
    }

    #[test]
    fn dev_mode_uses_defaults() {
        with_env(
            &[("DEV_MODE", "true")],
            &[
                "PORT",
                "BASE_URL",
                "ORIGIN_URL",
                "STITCHING_MODE",
                "VAST_ENDPOINT",
                "AD_PROVIDER_TYPE",
                "SESSION_STORE",
                "SESSION_TTL_SECS",
            ],
            || {
                let config = Config::from_env().expect("should succeed in dev mode");
                assert!(config.is_dev);
                assert_eq!(config.port, 3000);
                assert_eq!(config.base_url, "http://localhost:3000");
                assert_eq!(config.origin_url, "https://example.com");
                assert_eq!(config.stitching_mode, StitchingMode::Ssai);
                assert_eq!(config.ad_provider_type, AdProviderType::Static);
                assert_eq!(config.session_store, SessionStoreType::Memory);
                assert_eq!(config.session_ttl_secs, 300);
            },
        );
    }

    #[test]
    fn prod_mode_requires_port() {
        with_env(&[], &["DEV_MODE", "PORT", "BASE_URL", "ORIGIN_URL"], || {
            let result = Config::from_env();
            assert!(result.is_err(), "Should fail without PORT in prod mode");
        });
    }

    #[test]
    fn prod_mode_requires_base_url() {
        with_env(
            &[("PORT", "8080")],
            &["DEV_MODE", "BASE_URL", "ORIGIN_URL"],
            || {
                let result = Config::from_env();
                assert!(result.is_err(), "Should fail without BASE_URL in prod mode");
            },
        );
    }

    #[test]
    fn prod_mode_requires_origin_url() {
        with_env(
            &[("PORT", "8080"), ("BASE_URL", "https://example.com")],
            &["DEV_MODE", "ORIGIN_URL"],
            || {
                let result = Config::from_env();
                assert!(
                    result.is_err(),
                    "Should fail without ORIGIN_URL in prod mode"
                );
            },
        );
    }

    #[test]
    fn vast_auto_detect_from_endpoint() {
        with_env(
            &[
                ("DEV_MODE", "true"),
                ("VAST_ENDPOINT", "https://ads.example.com/vast"),
            ],
            &["AD_PROVIDER_TYPE"],
            || {
                let config = Config::from_env().unwrap();
                assert_eq!(config.ad_provider_type, AdProviderType::Vast);
                assert_eq!(
                    config.vast_endpoint,
                    Some("https://ads.example.com/vast".to_string())
                );
            },
        );
    }

    #[test]
    fn explicit_static_overrides_vast_endpoint() {
        with_env(
            &[
                ("DEV_MODE", "true"),
                ("VAST_ENDPOINT", "https://ads.example.com/vast"),
                ("AD_PROVIDER_TYPE", "static"),
            ],
            &[],
            || {
                let config = Config::from_env().unwrap();
                assert_eq!(config.ad_provider_type, AdProviderType::Static);
            },
        );
    }

    #[test]
    fn stitching_mode_sgai() {
        with_env(
            &[("DEV_MODE", "true"), ("STITCHING_MODE", "sgai")],
            &[],
            || {
                let config = Config::from_env().unwrap();
                assert_eq!(config.stitching_mode, StitchingMode::Sgai);
            },
        );
    }

    #[test]
    fn stitching_mode_defaults_to_ssai() {
        with_env(&[("DEV_MODE", "true")], &["STITCHING_MODE"], || {
            let config = Config::from_env().unwrap();
            assert_eq!(config.stitching_mode, StitchingMode::Ssai);
        });
    }

    #[test]
    fn session_store_valkey() {
        with_env(
            &[("DEV_MODE", "true"), ("SESSION_STORE", "valkey")],
            &[],
            || {
                let config = Config::from_env().unwrap();
                assert_eq!(config.session_store, SessionStoreType::Valkey);
            },
        );
    }

    #[test]
    fn session_store_redis_alias() {
        with_env(
            &[("DEV_MODE", "true"), ("SESSION_STORE", "redis")],
            &[],
            || {
                let config = Config::from_env().unwrap();
                assert_eq!(config.session_store, SessionStoreType::Valkey);
            },
        );
    }

    #[test]
    fn session_store_defaults_to_memory() {
        with_env(&[("DEV_MODE", "true")], &["SESSION_STORE"], || {
            let config = Config::from_env().unwrap();
            assert_eq!(config.session_store, SessionStoreType::Memory);
        });
    }

    #[test]
    fn session_ttl_parsed() {
        with_env(
            &[("DEV_MODE", "true"), ("SESSION_TTL_SECS", "600")],
            &[],
            || {
                let config = Config::from_env().unwrap();
                assert_eq!(config.session_ttl_secs, 600);
            },
        );
    }

    #[test]
    fn ad_segment_duration_parsed() {
        with_env(
            &[("DEV_MODE", "true"), ("AD_SEGMENT_DURATION", "2.5")],
            &[],
            || {
                let config = Config::from_env().unwrap();
                assert_eq!(config.ad_segment_duration, 2.5);
            },
        );
    }

    #[test]
    fn rate_limit_disabled_defaults_to_zero() {
        // When RATE_LIMIT_RPM is unset, rate_limit_rpm == 0 (disabled).
        // In prod mode this emits a tracing::warn! for operator visibility.
        with_env(
            &[
                ("PORT", "8080"),
                ("BASE_URL", "https://example.com"),
                ("ORIGIN_URL", "https://example.com"),
            ],
            &["DEV_MODE", "RATE_LIMIT_RPM"],
            || {
                let config = Config::from_env().unwrap();
                assert_eq!(config.rate_limit_rpm, 0);
            },
        );
    }

    #[test]
    fn vast_fallback_to_static_when_no_endpoint() {
        // When AD_PROVIDER_TYPE is "auto" (default) and no VAST_ENDPOINT is set,
        // ad_provider_type falls back to Static. In prod mode this emits a
        // tracing::warn! so operators know ads are served from static provider.
        with_env(
            &[
                ("PORT", "8080"),
                ("BASE_URL", "https://example.com"),
                ("ORIGIN_URL", "https://example.com"),
            ],
            &["DEV_MODE", "AD_PROVIDER_TYPE", "VAST_ENDPOINT"],
            || {
                let config = Config::from_env().unwrap();
                assert_eq!(config.ad_provider_type, AdProviderType::Static);
                assert!(config.vast_endpoint.is_none());
            },
        );
    }

    #[test]
    fn manifest_cache_ttl_defaults_to_2000() {
        with_env(&[("DEV_MODE", "true")], &["MANIFEST_CACHE_TTL_MS"], || {
            let config = Config::from_env().unwrap();
            assert_eq!(config.manifest_cache_ttl_ms, 2000);
        });
    }

    #[test]
    fn manifest_cache_ttl_custom_value() {
        with_env(
            &[("DEV_MODE", "true"), ("MANIFEST_CACHE_TTL_MS", "500")],
            &[],
            || {
                let config = Config::from_env().unwrap();
                assert_eq!(config.manifest_cache_ttl_ms, 500);
            },
        );
    }

    #[test]
    fn vast_explicit_without_endpoint_still_selects_vast() {
        // When AD_PROVIDER_TYPE is explicitly "vast" but no VAST_ENDPOINT is set,
        // the provider type is Vast (as requested) but a warning is emitted in prod.
        with_env(
            &[
                ("PORT", "8080"),
                ("BASE_URL", "https://example.com"),
                ("ORIGIN_URL", "https://example.com"),
                ("AD_PROVIDER_TYPE", "vast"),
            ],
            &["DEV_MODE", "VAST_ENDPOINT"],
            || {
                let config = Config::from_env().unwrap();
                assert_eq!(config.ad_provider_type, AdProviderType::Vast);
                assert!(config.vast_endpoint.is_none());
            },
        );
    }
}
