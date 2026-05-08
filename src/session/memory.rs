//! In-process [`SessionStore`] backed by a [`DashMap`].
//!
//! This is the default backend (no feature flag required). It is lock-free
//! for reads and uses sharded locks for writes; suitable for single-process
//! deployments and the dev server.
//!
//! TTL is enforced by [`SessionManager::cleanup_expired`](super::SessionManager::cleanup_expired)
//! calling [`MemoryStore::cleanup_expired`]; there is no background sweep.

use super::Session;
use super::store::{SessionError, SessionStore};
use async_trait::async_trait;
use dashmap::DashMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

/// `DashMap`-backed session store.
///
/// Cheap to clone (the inner map is `Arc`-shared), so multiple
/// [`SessionManager`](super::SessionManager) instances pointing at the same
/// store share state — though in practice only `AppState` holds one.
#[derive(Clone, Default)]
pub struct MemoryStore {
    sessions: Arc<DashMap<String, Session>>,
}

impl MemoryStore {
    /// Create an empty in-memory store.
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl SessionStore for MemoryStore {
    async fn get(&self, session_id: &str) -> Result<Option<Session>, SessionError> {
        Ok(self.sessions.get(session_id).map(|s| s.clone()))
    }

    async fn insert_if_absent(
        &self,
        session: Session,
        _ttl: Duration,
    ) -> Result<Session, SessionError> {
        // `entry().or_insert_with` is the atomic primitive on DashMap.
        let id = session.session_id.clone();
        let entry = self
            .sessions
            .entry(id)
            .or_insert_with(|| session.clone())
            .clone();
        Ok(entry)
    }

    async fn touch(&self, session_id: &str, _ttl: Duration) -> Result<(), SessionError> {
        if let Some(mut session) = self.sessions.get_mut(session_id) {
            session.last_accessed = SystemTime::now();
        }
        Ok(())
    }

    async fn remove(&self, session_id: &str) -> Result<Option<Session>, SessionError> {
        Ok(self.sessions.remove(session_id).map(|(_, s)| s))
    }

    async fn cleanup_expired(&self, ttl: Duration, now: SystemTime) -> Result<(), SessionError> {
        self.sessions.retain(|_, session| {
            if let Ok(elapsed) = now.duration_since(session.last_accessed) {
                elapsed < ttl
            } else {
                // Clock skew: keep the session rather than evict it.
                true
            }
        });
        Ok(())
    }

    async fn approx_count(&self) -> Result<usize, SessionError> {
        Ok(self.sessions.len())
    }
}
