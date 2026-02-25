use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

#[cfg(feature = "valkey")]
use tracing::{error, info};

#[cfg(feature = "valkey")]
use redis::aio::ConnectionManager;

/// Session data stored for each active session
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub session_id: String,
    pub origin_url: String,
    #[serde(with = "epoch_secs")]
    pub created_at: SystemTime,
    #[serde(with = "epoch_secs")]
    pub last_accessed: SystemTime,
}

/// Serde helper: SystemTime ↔ u64 epoch seconds
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

/// Internal storage backend
#[derive(Clone)]
enum Backend {
    Memory {
        sessions: Arc<DashMap<String, Session>>,
    },
    #[cfg(feature = "valkey")]
    Valkey {
        conn: ConnectionManager,
        key_prefix: String,
    },
}

/// Session manager — same public API regardless of backend
#[derive(Clone)]
pub struct SessionManager {
    backend: Backend,
    ttl: Duration,
}

impl SessionManager {
    /// Create an in-memory session manager (default)
    pub fn new_memory(ttl: Duration) -> Self {
        Self {
            backend: Backend::Memory {
                sessions: Arc::new(DashMap::new()),
            },
            ttl,
        }
    }

    /// Create a Valkey-backed session manager
    #[cfg(feature = "valkey")]
    pub async fn new_valkey(url: &str, ttl: Duration) -> Result<Self, redis::RedisError> {
        let client = redis::Client::open(url)?;
        let conn = ConnectionManager::new(client).await?;
        info!("Connected to Valkey at {}", url);
        Ok(Self {
            backend: Backend::Valkey {
                conn,
                key_prefix: "ritcher:session".to_string(),
            },
            ttl,
        })
    }

    /// Get or create a session
    pub async fn get_or_create(&self, session_id: String, origin_url: String) -> Session {
        match &self.backend {
            Backend::Memory { sessions } => sessions
                .entry(session_id.clone())
                .or_insert_with(|| {
                    let now = SystemTime::now();
                    Session {
                        session_id: session_id.clone(),
                        origin_url,
                        created_at: now,
                        last_accessed: now,
                    }
                })
                .clone(),
            #[cfg(feature = "valkey")]
            Backend::Valkey { conn, key_prefix } => {
                let key = format!("{}:{}", key_prefix, session_id);
                let mut conn = conn.clone();
                // Try to get existing session
                if let Ok(Some(json)) = redis::cmd("GET")
                    .arg(&key)
                    .query_async::<Option<String>>(&mut conn)
                    .await
                {
                    if let Ok(session) = serde_json::from_str::<Session>(&json) {
                        return session;
                    }
                }
                // Create new session
                let now = SystemTime::now();
                let session = Session {
                    session_id: session_id.clone(),
                    origin_url,
                    created_at: now,
                    last_accessed: now,
                };
                if let Ok(json) = serde_json::to_string(&session) {
                    let ttl_secs = self.ttl.as_secs();
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
                session
            }
        }
    }

    /// Update last accessed time for a session
    pub async fn touch(&self, session_id: &str) {
        match &self.backend {
            Backend::Memory { sessions } => {
                if let Some(mut session) = sessions.get_mut(session_id) {
                    session.last_accessed = SystemTime::now();
                }
            }
            #[cfg(feature = "valkey")]
            Backend::Valkey { conn, key_prefix } => {
                let key = format!("{}:{}", key_prefix, session_id);
                let mut conn = conn.clone();
                let ttl_secs = self.ttl.as_secs() as i64;
                // Use EXPIRE to refresh TTL in a single O(1) command instead of
                // GET → deserialize → modify → serialize → SET.
                // Trade-off: last_accessed is not updated in the stored JSON, but
                // the key's TTL accurately reflects session liveness. The field is
                // only used for diagnostics, not for eviction logic.
                if let Err(e) = redis::cmd("EXPIRE")
                    .arg(&key)
                    .arg(ttl_secs)
                    .query_async::<i32>(&mut conn)
                    .await
                {
                    error!("Valkey EXPIRE failed in touch: {}", e);
                }
            }
        }
    }

    /// Get a session by ID
    pub async fn get(&self, session_id: &str) -> Option<Session> {
        match &self.backend {
            Backend::Memory { sessions } => sessions.get(session_id).map(|s| s.clone()),
            #[cfg(feature = "valkey")]
            Backend::Valkey { conn, key_prefix } => {
                let key = format!("{}:{}", key_prefix, session_id);
                let mut conn = conn.clone();
                match redis::cmd("GET")
                    .arg(&key)
                    .query_async::<Option<String>>(&mut conn)
                    .await
                {
                    Ok(Some(json)) => serde_json::from_str(&json).ok(),
                    Ok(None) => None,
                    Err(e) => {
                        error!("Valkey GET failed: {}", e);
                        None
                    }
                }
            }
        }
    }

    /// Remove expired sessions (no-op for Valkey — TTL is native)
    pub async fn cleanup_expired(&self) {
        match &self.backend {
            Backend::Memory { sessions } => {
                let now = SystemTime::now();
                sessions.retain(|_, session| {
                    if let Ok(elapsed) = now.duration_since(session.last_accessed) {
                        elapsed < self.ttl
                    } else {
                        true
                    }
                });
            }
            #[cfg(feature = "valkey")]
            Backend::Valkey { .. } => {
                // Valkey handles TTL natively via EXPIRE — nothing to do
            }
        }
    }

    /// Get the count of active sessions
    pub async fn session_count(&self) -> usize {
        match &self.backend {
            Backend::Memory { sessions } => sessions.len(),
            #[cfg(feature = "valkey")]
            Backend::Valkey { conn, key_prefix } => {
                let pattern = format!("{}:*", key_prefix);
                let mut conn = conn.clone();
                // Use SCAN instead of KEYS to avoid blocking Valkey.
                // SCAN is cursor-based and yields control between batches.
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
                            error!("Valkey SCAN failed in session_count: {}", e);
                            return 0;
                        }
                    };
                    count += result.1.len();
                    cursor = result.0;
                    if cursor == 0 {
                        break;
                    }
                }
                count
            }
        }
    }

    /// Remove a specific session
    pub async fn remove(&self, session_id: &str) -> Option<Session> {
        match &self.backend {
            Backend::Memory { sessions } => sessions.remove(session_id).map(|(_, session)| session),
            #[cfg(feature = "valkey")]
            Backend::Valkey { conn, key_prefix } => {
                let key = format!("{}:{}", key_prefix, session_id);
                let mut conn = conn.clone();
                // GET then DEL
                let json: Option<String> =
                    match redis::cmd("GET").arg(&key).query_async(&mut conn).await {
                        Ok(v) => v,
                        Err(e) => {
                            error!("Valkey GET failed in remove: {}", e);
                            return None;
                        }
                    };
                if json.is_some() {
                    if let Err(e) = redis::cmd("DEL")
                        .arg(&key)
                        .query_async::<()>(&mut conn)
                        .await
                    {
                        error!("Valkey DEL failed in remove: {}", e);
                    }
                }
                json.and_then(|j| serde_json::from_str(&j).ok())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
