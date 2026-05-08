use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::{Duration, SystemTime};
use tracing::warn;

use super::memory::MemoryStore;
use super::store::SessionStore;

/// Session data stored for each active session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub session_id: String,
    pub origin_url: String,
    #[serde(with = "epoch_secs")]
    pub created_at: SystemTime,
    #[serde(with = "epoch_secs")]
    pub last_accessed: SystemTime,
}

/// Serde helper: SystemTime ↔ u64 epoch seconds.
mod epoch_secs {
    use serde::{Deserialize, Deserializer, Serializer};
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    pub fn serialize<S>(time: &SystemTime, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let secs = time
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        serializer.serialize_u64(secs)
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<SystemTime, D::Error>
    where
        D: Deserializer<'de>,
    {
        let secs = u64::deserialize(deserializer)?;
        Ok(UNIX_EPOCH + Duration::from_secs(secs))
    }
}

/// Session manager — the public API for session lifecycle.
///
/// The manager holds an `Arc<dyn SessionStore>` so callers don't need to
/// know whether sessions live in process memory or in Valkey. Construct
/// with [`SessionManager::new_memory`] or [`SessionManager::new_valkey`]
/// (the latter requires the `valkey` cargo feature).
///
/// `Clone` is cheap: cloning bumps the `Arc` refcount on the store.
#[derive(Clone)]
pub struct SessionManager {
    store: Arc<dyn SessionStore>,
    ttl: Duration,
}

impl SessionManager {
    /// Internal constructor for tests; external callers must use `new_memory`
    /// or `new_valkey`.
    pub(crate) fn from_store(store: Arc<dyn SessionStore>, ttl: Duration) -> Self {
        Self { store, ttl }
    }

    /// Create an in-memory session manager (default).
    pub fn new_memory(ttl: Duration) -> Self {
        Self::from_store(Arc::new(MemoryStore::new()), ttl)
    }

    // `new_valkey` lives in [`super::valkey`] as a feature-gated inherent
    // impl on [`SessionManager`], so the `valkey` cargo feature does not
    // leak into this file.

    /// Get or create a session.
    ///
    /// If a session with `session_id` already exists, the existing record is
    /// returned unchanged (the supplied `origin_url` is ignored — this is the
    /// idempotent path covered by `get_or_create_returns_existing_session`).
    pub async fn get_or_create(&self, session_id: String, origin_url: String) -> Session {
        let now = SystemTime::now();
        let candidate = Session {
            session_id: session_id.clone(),
            origin_url,
            created_at: now,
            last_accessed: now,
        };
        match self
            .store
            .insert_if_absent(candidate.clone(), self.ttl)
            .await
        {
            Ok(session) => session,
            Err(e) => {
                // Backend failure: fall back to the candidate so the caller
                // still gets a usable session record. The next request will
                // retry the store and either hit the existing record or
                // re-insert.
                warn!("session store insert_if_absent failed: {}", e);
                candidate
            }
        }
    }

    /// Update last accessed time for a session.
    ///
    /// Memory backends mutate `last_accessed`; Valkey backends refresh native
    /// TTL via `EXPIRE` without rewriting the JSON. Either way, the session's
    /// effective liveness window is extended by `ttl`.
    pub async fn touch(&self, session_id: &str) {
        if let Err(e) = self.store.touch(session_id, self.ttl).await {
            warn!("session store touch failed for {}: {}", session_id, e);
        }
    }

    /// Get a session by ID.
    pub async fn get(&self, session_id: &str) -> Option<Session> {
        match self.store.get(session_id).await {
            Ok(s) => s,
            Err(e) => {
                warn!("session store get failed for {}: {}", session_id, e);
                None
            }
        }
    }

    /// Remove expired sessions (no-op for backends with native TTL).
    pub async fn cleanup_expired(&self) {
        if let Err(e) = self
            .store
            .cleanup_expired(self.ttl, SystemTime::now())
            .await
        {
            warn!("session store cleanup_expired failed: {}", e);
        }
    }

    /// Get the count of active sessions.
    ///
    /// **Approximated on the Valkey backend**: the count is collected via a
    /// cursor-based `SCAN` walk and is not exact under concurrent
    /// inserts/expirations during the walk. The in-memory backend returns
    /// an exact count. Either way, this is intended for diagnostics
    /// (e.g. the `/health` endpoint), not for correctness-sensitive logic.
    ///
    /// See [`SessionStore::approx_count`](super::store::SessionStore::approx_count)
    /// for backend specifics.
    pub async fn session_count(&self) -> usize {
        match self.store.approx_count().await {
            Ok(n) => n,
            Err(e) => {
                warn!("session store approx_count failed: {}", e);
                0
            }
        }
    }

    /// Remove a specific session, returning the removed record if it existed.
    pub async fn remove(&self, session_id: &str) -> Option<Session> {
        match self.store.remove(session_id).await {
            Ok(s) => s,
            Err(e) => {
                warn!("session store remove failed for {}: {}", session_id, e);
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::store::{SessionError, SessionStore};
    use async_trait::async_trait;
    use std::sync::Mutex;

    /// Minimal in-process fake used to demonstrate that the store seam
    /// makes test doubles trivial. Backed by a `Mutex<Vec<_>>` so we can
    /// also count call interactions.
    struct FakeSessionStore {
        sessions: Mutex<Vec<Session>>,
        get_calls: Mutex<usize>,
    }

    impl FakeSessionStore {
        fn new() -> Self {
            Self {
                sessions: Mutex::new(Vec::new()),
                get_calls: Mutex::new(0),
            }
        }

        fn get_call_count(&self) -> usize {
            *self.get_calls.lock().unwrap()
        }
    }

    #[async_trait]
    impl SessionStore for FakeSessionStore {
        async fn get(&self, session_id: &str) -> Result<Option<Session>, SessionError> {
            *self.get_calls.lock().unwrap() += 1;
            let sessions = self.sessions.lock().unwrap();
            Ok(sessions
                .iter()
                .find(|s| s.session_id == session_id)
                .cloned())
        }

        async fn insert_if_absent(
            &self,
            session: Session,
            _ttl: Duration,
        ) -> Result<Session, SessionError> {
            let resolved = {
                let mut sessions = self.sessions.lock().unwrap();
                if let Some(existing) = sessions.iter().find(|s| s.session_id == session.session_id)
                {
                    existing.clone()
                } else {
                    sessions.push(session.clone());
                    session
                }
            };
            Ok(resolved)
        }

        async fn touch(&self, session_id: &str, _ttl: Duration) -> Result<(), SessionError> {
            {
                let mut sessions = self.sessions.lock().unwrap();
                if let Some(s) = sessions.iter_mut().find(|s| s.session_id == session_id) {
                    s.last_accessed = SystemTime::now();
                }
            }
            Ok(())
        }

        async fn remove(&self, session_id: &str) -> Result<Option<Session>, SessionError> {
            let removed = {
                let mut sessions = self.sessions.lock().unwrap();
                sessions
                    .iter()
                    .position(|s| s.session_id == session_id)
                    .map(|pos| sessions.remove(pos))
            };
            Ok(removed)
        }

        async fn cleanup_expired(
            &self,
            ttl: Duration,
            now: SystemTime,
        ) -> Result<(), SessionError> {
            {
                let mut sessions = self.sessions.lock().unwrap();
                sessions.retain(|s| {
                    now.duration_since(s.last_accessed)
                        .map(|elapsed| elapsed < ttl)
                        .unwrap_or(true)
                });
            }
            Ok(())
        }

        async fn approx_count(&self) -> Result<usize, SessionError> {
            Ok(self.sessions.lock().unwrap().len())
        }
    }

    #[tokio::test]
    async fn test_session_creation() {
        let manager = SessionManager::new_memory(Duration::from_secs(300));
        let session = manager
            .get_or_create("test123".to_string(), "https://example.com".to_string())
            .await;

        assert_eq!(session.session_id, "test123");
        assert_eq!(session.origin_url, "https://example.com");
        assert_eq!(manager.session_count().await, 1);
    }

    #[tokio::test]
    async fn test_session_touch() {
        let manager = SessionManager::new_memory(Duration::from_secs(300));
        let session = manager
            .get_or_create("test456".to_string(), "https://example.com".to_string())
            .await;

        let initial_time = session.last_accessed;
        std::thread::sleep(Duration::from_millis(10));
        manager.touch("test456").await;

        let updated_session = manager.get("test456").await.unwrap();
        assert!(updated_session.last_accessed > initial_time);
    }

    #[tokio::test]
    async fn test_session_removal() {
        let manager = SessionManager::new_memory(Duration::from_secs(300));
        manager
            .get_or_create("test789".to_string(), "https://example.com".to_string())
            .await;

        assert_eq!(manager.session_count().await, 1);
        manager.remove("test789").await;
        assert_eq!(manager.session_count().await, 0);
    }

    #[tokio::test]
    async fn session_count_empty() {
        let manager = SessionManager::new_memory(Duration::from_secs(300));
        assert_eq!(manager.session_count().await, 0);
    }

    #[tokio::test]
    async fn get_nonexistent_returns_none() {
        let manager = SessionManager::new_memory(Duration::from_secs(300));
        assert!(manager.get("no-such-session").await.is_none());
    }

    #[tokio::test]
    async fn remove_nonexistent_returns_none() {
        let manager = SessionManager::new_memory(Duration::from_secs(300));
        assert!(manager.remove("no-such-session").await.is_none());
    }

    #[tokio::test]
    async fn get_or_create_returns_existing_session() {
        let manager = SessionManager::new_memory(Duration::from_secs(300));
        manager
            .get_or_create("idempotent".to_string(), "https://first.com".to_string())
            .await;
        // Second call with a different origin_url — existing session should be returned
        let session = manager
            .get_or_create("idempotent".to_string(), "https://second.com".to_string())
            .await;
        assert_eq!(
            session.origin_url, "https://first.com",
            "Should return existing session, not create a new one"
        );
        assert_eq!(manager.session_count().await, 1);
    }

    #[tokio::test]
    async fn cleanup_expired_removes_stale_sessions() {
        // Very short TTL so sessions expire almost immediately.
        let manager = SessionManager::new_memory(Duration::from_millis(1));
        manager
            .get_or_create("stale".to_string(), "https://example.com".to_string())
            .await;
        assert_eq!(manager.session_count().await, 1);

        // Wait for TTL to elapse, then clean up.
        tokio::time::sleep(Duration::from_millis(5)).await;
        manager.cleanup_expired().await;

        assert_eq!(
            manager.session_count().await,
            0,
            "Stale session should be removed"
        );
    }

    #[tokio::test]
    async fn fake_store_drives_manager() {
        // Demonstrates that the trait seam makes test doubles trivial:
        // no DashMap, no Valkey, no feature flags — just a plain Mutex<Vec<_>>.
        let fake = Arc::new(FakeSessionStore::new());
        let manager = SessionManager::from_store(fake.clone(), Duration::from_secs(60));

        manager
            .get_or_create("fake".to_string(), "https://fake.test".to_string())
            .await;
        assert_eq!(manager.session_count().await, 1);

        let s = manager.get("fake").await.expect("should be present");
        assert_eq!(s.origin_url, "https://fake.test");
        assert_eq!(
            fake.get_call_count(),
            1,
            "exactly one underlying GET should have been issued"
        );

        let removed = manager.remove("fake").await;
        assert!(removed.is_some());
        assert_eq!(manager.session_count().await, 0);
    }
}
