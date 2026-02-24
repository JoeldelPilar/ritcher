//! HTTP fetch with automatic retry and backoff.
//!
//! Provides [`fetch_with_retry`] to deduplicate the retry pattern that was
//! previously copy-pasted in `handlers/ad.rs`, `handlers/segment.rs`, and
//! `ad/vast_provider.rs`.

use reqwest::{Client, Response};
use std::time::Duration;
use tracing::warn;

/// Default number of fetch attempts (1 initial + 1 retry).
pub const DEFAULT_MAX_ATTEMPTS: u32 = 2;

/// Default backoff between attempts in milliseconds.
pub const DEFAULT_BACKOFF_MS: u64 = 500;

/// Configuration for [`fetch_with_retry`].
pub struct RetryConfig {
    /// Total number of attempts (minimum 1; 0 is treated as 1).
    pub max_attempts: u32,
    /// Sleep duration between consecutive attempts.
    pub backoff: Duration,
    /// Optional per-request timeout applied to each individual attempt.
    ///
    /// When `None`, the client's own timeout applies.
    pub timeout: Option<Duration>,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            max_attempts: DEFAULT_MAX_ATTEMPTS,
            backoff: Duration::from_millis(DEFAULT_BACKOFF_MS),
            timeout: None,
        }
    }
}

/// Fetch a URL via HTTP GET with automatic retry and backoff.
///
/// Attempts the request up to `config.max_attempts` times, sleeping
/// `config.backoff` between each attempt.
///
/// Returns the first successful (2xx) [`Response`], or the last
/// [`reqwest::Error`] encountered once all attempts are exhausted.
///
/// # Errors
///
/// Returns the last network or non-2xx error after all retries fail.
pub async fn fetch_with_retry(
    client: &Client,
    url: &str,
    config: &RetryConfig,
) -> Result<Response, reqwest::Error> {
    let max_attempts = config.max_attempts.max(1);

    for attempt in 1..=max_attempts {
        let is_last = attempt == max_attempts;

        let mut request = client.get(url);
        if let Some(timeout) = config.timeout {
            request = request.timeout(timeout);
        }

        match request.send().await {
            Ok(response) if response.status().is_success() => return Ok(response),

            Ok(response) => {
                warn!(
                    "HTTP fetch returned {} for {} (attempt {}/{})",
                    response.status(),
                    url,
                    attempt,
                    max_attempts
                );
                let err = response.error_for_status().unwrap_err();
                if is_last {
                    return Err(err);
                }
            }

            Err(e) => {
                warn!(
                    "HTTP fetch failed for {} (attempt {}/{}): {}",
                    url, attempt, max_attempts, e
                );
                if is_last {
                    return Err(e);
                }
            }
        }

        warn!("Retrying HTTP fetch in {}ms...", config.backoff.as_millis());
        tokio::time::sleep(config.backoff).await;
    }

    unreachable!(
        "fetch_with_retry: exhausted {} attempt(s) without returning — this is a bug",
        max_attempts
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retry_config_defaults() {
        let cfg = RetryConfig::default();
        assert_eq!(cfg.max_attempts, DEFAULT_MAX_ATTEMPTS);
        assert_eq!(cfg.backoff, Duration::from_millis(DEFAULT_BACKOFF_MS));
        assert!(cfg.timeout.is_none());
    }

    #[test]
    fn retry_config_custom() {
        let cfg = RetryConfig {
            max_attempts: 5,
            backoff: Duration::from_millis(100),
            timeout: Some(Duration::from_secs(10)),
        };
        assert_eq!(cfg.max_attempts, 5);
        assert_eq!(cfg.backoff, Duration::from_millis(100));
        assert_eq!(cfg.timeout, Some(Duration::from_secs(10)));
    }

    #[test]
    fn max_attempts_zero_treated_as_one() {
        let cfg = RetryConfig {
            max_attempts: 0,
            ..Default::default()
        };
        // max(1) guard ensures at least one attempt
        assert_eq!(cfg.max_attempts.max(1), 1);
    }
}
