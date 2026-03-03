use crate::{
    ad::{AdProvider, DemoAdProvider, SlateProvider, StaticAdProvider, VastAdProvider},
    cache::ManifestCache,
    config::{AdProviderType, Config, SessionStoreType},
    server::{
        dns_resolver::SsrfSafeResolver, rate_limit::RateLimiter,
        url_validation::validate_origin_url,
    },
    session::SessionManager,
};
use reqwest::{Client, redirect};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tracing::{info, warn};

/// Application state shared across all handlers
#[derive(Clone)]
pub struct AppState {
    /// Application configuration
    pub config: Arc<Config>,
    /// Shared HTTP client for connection pooling
    pub http_client: Client,
    /// Session manager for tracking active sessions
    pub sessions: SessionManager,
    /// Ad provider for serving ad content (trait object for runtime flexibility)
    pub ad_provider: Arc<dyn AdProvider>,
    /// Short-TTL cache for origin manifests (deduplicates concurrent fetches)
    pub manifest_cache: ManifestCache,
    /// Optional per-IP rate limiter (None when RATE_LIMIT_RPM=0)
    pub rate_limiter: Option<RateLimiter>,
    /// Server start time for uptime tracking
    pub started_at: Instant,
}

impl AppState {
    /// Create a new AppState with the given configuration
    pub async fn new(config: Config) -> Self {
        // Custom redirect policy: re-validate each redirect target against
        // SSRF rules. Without this, an attacker can point an origin URL at a
        // public host that 3xx-redirects to a private/internal IP, bypassing
        // the initial validate_origin_url() check.
        let ssrf_safe_redirect = redirect::Policy::custom(|attempt| {
            // Cap total redirects at 10 (same as reqwest default)
            if attempt.previous().len() >= 10 {
                attempt.error(std::io::Error::other("too many redirects"))
            } else {
                let target = attempt.url().to_string();
                match validate_origin_url(&target) {
                    Ok(()) => attempt.follow(),
                    Err(_) => {
                        warn!(
                            "SSRF: blocked redirect to {} (from {:?})",
                            target,
                            attempt.previous().last()
                        );
                        attempt.error(std::io::Error::other(format!(
                            "SSRF: redirect to blocked address {}",
                            target
                        )))
                    }
                }
            }
        });

        let builder = Client::builder()
            .redirect(ssrf_safe_redirect)
            .timeout(Duration::from_secs(config.origin_timeout_secs))
            .connect_timeout(Duration::from_secs(5))
            .pool_idle_timeout(Duration::from_secs(90))
            .pool_max_idle_per_host(10);

        // In production, use SSRF-safe DNS resolver to prevent DNS rebinding
        // attacks. In dev mode, wiremock binds to 127.0.0.1 which the resolver
        // would block, so we skip it.
        let builder = if !config.is_dev {
            info!("DNS rebinding protection: enabled (SSRF-safe resolver)");
            builder.dns_resolver(Arc::new(SsrfSafeResolver::new()))
        } else {
            info!("DNS rebinding protection: disabled (dev mode)");
            builder
        };

        let http_client = builder.build().expect("Failed to create HTTP client");

        let ttl = Duration::from_secs(config.session_ttl_secs);
        let sessions = match config.session_store {
            SessionStoreType::Memory => SessionManager::new_memory(ttl),
            #[cfg(feature = "valkey")]
            SessionStoreType::Valkey => {
                let url = config
                    .valkey_url
                    .as_deref()
                    .expect("VALKEY_URL is required when SESSION_STORE=valkey");
                SessionManager::new_valkey(url, ttl)
                    .await
                    .expect("Failed to connect to Valkey")
            }
            #[cfg(not(feature = "valkey"))]
            SessionStoreType::Valkey => {
                panic!("SESSION_STORE=valkey requires the 'valkey' feature flag");
            }
        };

        // Create ad provider based on config
        let ad_provider: Arc<dyn AdProvider> = match config.ad_provider_type {
            AdProviderType::Vast => {
                let endpoint = config
                    .vast_endpoint
                    .as_deref()
                    .expect("VAST_ENDPOINT is required when AD_PROVIDER_TYPE=vast");
                info!("Ad provider: VAST (endpoint: {})", endpoint);

                let mut provider = VastAdProvider::new(endpoint.to_string(), http_client.clone());

                // Configure slate fallback if SLATE_URL is set
                if let Some(slate_url) = &config.slate_url {
                    info!(
                        "Slate fallback: enabled (url: {}, segment duration: {}s)",
                        slate_url, config.slate_segment_duration
                    );
                    provider = provider.with_slate(SlateProvider::new(
                        slate_url.clone(),
                        config.slate_segment_duration,
                    ));
                } else {
                    info!("Slate fallback: disabled (no SLATE_URL configured)");
                }

                Arc::new(provider)
            }
            AdProviderType::Static => {
                info!(
                    "Ad provider: Static (source: {}, segment duration: {}s)",
                    config.ad_source_url, config.ad_segment_duration
                );
                Arc::new(StaticAdProvider::new(
                    config.ad_source_url.clone(),
                    config.ad_segment_duration,
                ))
            }
            AdProviderType::Demo => {
                let base_url = config
                    .demo_ad_base_url
                    .as_deref()
                    .unwrap_or("http://localhost:3333/ads");
                info!(
                    "Ad provider: Demo ({} creatives at {})",
                    DemoAdProvider::NUM_CREATIVES,
                    base_url
                );
                Arc::new(DemoAdProvider::new(base_url))
            }
        };

        let rate_limiter = if config.rate_limit_rpm > 0 {
            info!(
                "Rate limiter: {} requests/min per IP",
                config.rate_limit_rpm
            );
            Some(RateLimiter::new(config.rate_limit_rpm))
        } else {
            info!("Rate limiter: disabled");
            None
        };

        let manifest_cache =
            ManifestCache::with_ttl(Duration::from_millis(config.manifest_cache_ttl_ms));

        Self {
            config: Arc::new(config),
            http_client,
            sessions,
            ad_provider,
            manifest_cache,
            rate_limiter,
            started_at: Instant::now(),
        }
    }
}
