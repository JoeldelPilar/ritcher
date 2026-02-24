use crate::{
    ad::{AdProvider, SlateProvider, StaticAdProvider, VastAdProvider},
    config::{AdProviderType, Config, SessionStoreType},
    session::SessionManager,
};
use reqwest::Client;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tracing::info;

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
    /// Server start time for uptime tracking
    pub started_at: Instant,
}

impl AppState {
    /// Create a new AppState with the given configuration
    pub async fn new(config: Config) -> Self {
        let http_client = Client::builder()
            .timeout(Duration::from_secs(30))
            .connect_timeout(Duration::from_secs(5))
            .pool_idle_timeout(Duration::from_secs(90))
            .pool_max_idle_per_host(10)
            .build()
            .expect("Failed to create HTTP client");

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
        };

        Self {
            config: Arc::new(config),
            http_client,
            sessions,
            ad_provider,
            started_at: Instant::now(),
        }
    }
}
