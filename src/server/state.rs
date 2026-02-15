use crate::{config::Config, session::SessionManager};
use reqwest::Client;
use std::sync::Arc;
use std::time::Duration;

/// Application state shared across all handlers
#[derive(Clone)]
pub struct AppState {
    /// Application configuration
    pub config: Arc<Config>,
    /// Shared HTTP client for connection pooling
    pub http_client: Client,
    /// Session manager for tracking active sessions
    pub sessions: SessionManager,
}

impl AppState {
    /// Create a new AppState with the given configuration
    pub fn new(config: Config) -> Self {
        let http_client = Client::builder()
            .pool_idle_timeout(Duration::from_secs(90))
            .pool_max_idle_per_host(10)
            .build()
            .expect("Failed to create HTTP client");

        // Session TTL: 5 minutes
        let sessions = SessionManager::new(Duration::from_secs(300));

        Self {
            config: Arc::new(config),
            http_client,
            sessions,
        }
    }
}
