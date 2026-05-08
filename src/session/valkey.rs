//! Valkey/Redis-backed [`SessionStore`] for distributed deployments.
//!
//! Enabled by the `valkey` cargo feature. Sessions are stored as JSON under
//! `ritcher:session:<id>` with native TTL via `SET ... EX`. The whole module
//! is `#[cfg(feature = "valkey")]` at the import site — no feature gates leak
//! into [`super::manager`].
//!
//! ## Operational notes
//!
//! - `touch` issues `EXPIRE` only; the JSON `last_accessed` field is **not**
//!   refreshed. The field is for diagnostics, not eviction logic, and the
//!   single-command path is hot.
//! - `approx_count` walks keys with `SCAN` (cursor-based, non-blocking) rather
//!   than `KEYS` to avoid stalling the server on large keyspaces. Concurrent
//!   inserts/expirations during the walk may be missed.

use super::Session;
use super::manager::SessionManager;
use super::store::{SessionError, SessionStore};
use async_trait::async_trait;
use redis::aio::ConnectionManager;
use std::sync::Arc;
use std::time::{Duration, SystemTime};
use tracing::{error, info};

const KEY_PREFIX: &str = "ritcher:session";

impl SessionManager {
    /// Create a Valkey-backed session manager.
    ///
    /// This constructor is defined here (not in `manager.rs`) so the
    /// `valkey` cargo feature gate stays inside the valkey module.
    pub async fn new_valkey(url: &str, ttl: Duration) -> Result<Self, redis::RedisError> {
        let store = ValkeyStore::connect(url).await?;
        Ok(Self::from_store(Arc::new(store), ttl))
    }
}

/// Valkey-backed session store. Cheap to clone — wraps a pooled
/// [`ConnectionManager`].
#[derive(Clone)]
pub struct ValkeyStore {
    conn: ConnectionManager,
    key_prefix: String,
}

impl ValkeyStore {
    /// Connect to Valkey at `url` and return a ready store.
    ///
    /// The connection is wrapped in [`ConnectionManager`] for automatic
    /// reconnection. Errors here are fatal at startup and should panic at
    /// the call site (see [`super::SessionManager::new_valkey`]).
    pub async fn connect(url: &str) -> Result<Self, redis::RedisError> {
        let client = redis::Client::open(url)?;
        let conn = ConnectionManager::new(client).await?;
        info!("Connected to Valkey at {}", url);
        Ok(Self {
            conn,
            key_prefix: KEY_PREFIX.to_string(),
        })
    }

    fn key_for(&self, session_id: &str) -> String {
        format!("{}:{}", self.key_prefix, session_id)
    }
}

#[async_trait]
impl SessionStore for ValkeyStore {
    async fn get(&self, session_id: &str) -> Result<Option<Session>, SessionError> {
        let key = self.key_for(session_id);
        let mut conn = self.conn.clone();
        match redis::cmd("GET")
            .arg(&key)
            .query_async::<Option<String>>(&mut conn)
            .await
        {
            Ok(Some(json)) => Ok(serde_json::from_str(&json).ok()),
            Ok(None) => Ok(None),
            Err(e) => {
                error!("Valkey GET failed: {}", e);
                Err(SessionError::Backend(e.to_string()))
            }
        }
    }

    async fn insert_if_absent(
        &self,
        session: Session,
        ttl: Duration,
    ) -> Result<Session, SessionError> {
        let key = self.key_for(&session.session_id);
        let mut conn = self.conn.clone();

        // Optimistic read: most calls hit existing sessions in steady state.
        if let Ok(Some(json)) = redis::cmd("GET")
            .arg(&key)
            .query_async::<Option<String>>(&mut conn)
            .await
            && let Ok(existing) = serde_json::from_str::<Session>(&json)
        {
            return Ok(existing);
        }

        // Miss (or corrupt JSON): write the new record. `SET ... EX` is not
        // atomic w.r.t. the GET above; a concurrent racer can win and we may
        // overwrite. This matches the prior behaviour exactly — switching to
        // `SET NX` would change semantics and is out of scope for this
        // refactor.
        if let Ok(json) = serde_json::to_string(&session) {
            let ttl_secs = ttl.as_secs();
            if let Err(e) = redis::cmd("SET")
                .arg(&key)
                .arg(&json)
                .arg("EX")
                .arg(ttl_secs)
                .query_async::<()>(&mut conn)
                .await
            {
                error!("Failed to store session in Valkey: {}", e);
            }
        }
        Ok(session)
    }

    async fn touch(&self, session_id: &str, ttl: Duration) -> Result<(), SessionError> {
        let key = self.key_for(session_id);
        let mut conn = self.conn.clone();
        // Session TTL is configured in seconds (default 300); u64 -> i64 is
        // safe for any realistic TTL value (max ~292 billion years).
        #[allow(clippy::cast_possible_truncation)]
        let ttl_secs = ttl.as_secs() as i64;
        // EXPIRE is O(1). We deliberately do not GET → mutate → SET to keep
        // the hot path single-roundtrip; `last_accessed` in stored JSON is
        // diagnostic only.
        if let Err(e) = redis::cmd("EXPIRE")
            .arg(&key)
            .arg(ttl_secs)
            .query_async::<i32>(&mut conn)
            .await
        {
            error!("Valkey EXPIRE failed in touch: {}", e);
            return Err(SessionError::Backend(e.to_string()));
        }
        Ok(())
    }

    async fn remove(&self, session_id: &str) -> Result<Option<Session>, SessionError> {
        let key = self.key_for(session_id);
        let mut conn = self.conn.clone();
        // GET then DEL — preserves prior behaviour of returning the removed
        // session record to the caller. Two roundtrips, but `remove` is
        // not on the steady-state hot path.
        let json: Option<String> = match redis::cmd("GET").arg(&key).query_async(&mut conn).await {
            Ok(v) => v,
            Err(e) => {
                error!("Valkey GET failed in remove: {}", e);
                return Err(SessionError::Backend(e.to_string()));
            }
        };
        if json.is_some()
            && let Err(e) = redis::cmd("DEL")
                .arg(&key)
                .query_async::<()>(&mut conn)
                .await
        {
            error!("Valkey DEL failed in remove: {}", e);
        }
        Ok(json.and_then(|j| serde_json::from_str(&j).ok()))
    }

    async fn cleanup_expired(&self, _ttl: Duration, _now: SystemTime) -> Result<(), SessionError> {
        // Valkey enforces TTL natively via `EX` on `SET`. Nothing to sweep.
        Ok(())
    }

    async fn approx_count(&self) -> Result<usize, SessionError> {
        let pattern = format!("{}:*", self.key_prefix);
        let mut conn = self.conn.clone();
        // SCAN is cursor-based and yields control between batches, so it
        // does not block other Valkey clients the way `KEYS` would.
        let mut cursor: u64 = 0;
        let mut count: usize = 0;
        loop {
            let result: (u64, Vec<String>) = match redis::cmd("SCAN")
                .arg(cursor)
                .arg("MATCH")
                .arg(&pattern)
                .arg("COUNT")
                .arg(100)
                .query_async(&mut conn)
                .await
            {
                Ok(r) => r,
                Err(e) => {
                    error!("Valkey SCAN failed in approx_count: {}", e);
                    return Err(SessionError::Backend(e.to_string()));
                }
            };
            count += result.1.len();
            cursor = result.0;
            if cursor == 0 {
                break;
            }
        }
        Ok(count)
    }
}
