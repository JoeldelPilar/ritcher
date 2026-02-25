use std::env;

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
}

/// Application configuration loaded from environment variables
#[derive(Clone, Debug)]
pub struct Config {
    pub port: u16,
    pub base_url: String,
    pub origin_url: String,
    pub is_dev: bool,
    /// HLS stitching mode: ssai (default) or sgai
    pub stitching_mode: StitchingMode,
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
    /// Session store backend
    pub session_store: SessionStoreType,
    /// Valkey/Redis URL (used when session_store = Valkey)
    pub valkey_url: Option<String>,
    /// Session TTL in seconds (default: 300)
    pub session_ttl_secs: u64,
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
}
