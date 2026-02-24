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
                let json: Option<String> =
                    match redis::cmd("GET").arg(&key).query_async(&mut conn).await {
                        Ok(v) => v,
                        Err(e) => {
                            error!("Valkey GET failed in touch: {}", e);
                            return;
                        }
                    };
                if let Some(json) = json {
                    if let Ok(mut session) = serde_json::from_str::<Session>(&json) {
                        session.last_accessed = SystemTime::now();
                        if let Ok(updated) = serde_json::to_string(&session) {
                            let ttl_secs = self.ttl.as_secs();
                            if let Err(e) = redis::cmd("SET")
                                .arg(&key)
                                .arg(&updated)
                                .arg("EX")
                                .arg(ttl_secs)
                                .query_async::<()>(&mut conn)
                                .await
                            {
                                error!("Valkey SET failed in touch: {}", e);
                            }
                        }
                    }
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
                // NOTE: KEYS is O(N) — acceptable for health endpoint at current scale.
                // Replace with SCAN or atomic counter if session volume exceeds ~10k.
                match redis::cmd("KEYS")
                    .arg(&pattern)
                    .query_async::<Vec<String>>(&mut conn)
                    .await
                {
                    Ok(keys) => keys.len(),
                    Err(e) => {
                        error!("Valkey KEYS failed in session_count: {}", e);
                        0
                    }
                }
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
}
