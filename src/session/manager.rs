use dashmap::DashMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

/// Session data stored for each active session
#[derive(Debug, Clone)]
pub struct Session {
    pub session_id: String,
    pub origin_url: String,
    pub created_at: SystemTime,
    pub last_accessed: SystemTime,
}

/// Session manager using DashMap for concurrent access
#[derive(Clone)]
pub struct SessionManager {
    sessions: Arc<DashMap<String, Session>>,
    ttl: Duration,
}

impl SessionManager {
    /// Create a new SessionManager with specified TTL
    pub fn new(ttl: Duration) -> Self {
        Self {
            sessions: Arc::new(DashMap::new()),
            ttl,
        }
    }

    /// Get or create a session
    pub fn get_or_create(&self, session_id: String, origin_url: String) -> Session {
        self.sessions
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
            .clone()
    }

    /// Update last accessed time for a session
    pub fn touch(&self, session_id: &str) {
        if let Some(mut session) = self.sessions.get_mut(session_id) {
            session.last_accessed = SystemTime::now();
        }
    }

    /// Get a session by ID
    pub fn get(&self, session_id: &str) -> Option<Session> {
        self.sessions.get(session_id).map(|s| s.clone())
    }

    /// Remove expired sessions (TTL cleanup)
    pub fn cleanup_expired(&self) {
        let now = SystemTime::now();
        self.sessions.retain(|_, session| {
            if let Ok(elapsed) = now.duration_since(session.last_accessed) {
                elapsed < self.ttl
            } else {
                true // Keep if we can't determine elapsed time
            }
        });
    }

    /// Get the count of active sessions
    pub fn session_count(&self) -> usize {
        self.sessions.len()
    }

    /// Remove a specific session
    pub fn remove(&self, session_id: &str) -> Option<Session> {
        self.sessions.remove(session_id).map(|(_, session)| session)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_session_creation() {
        let manager = SessionManager::new(Duration::from_secs(300));
        let session =
            manager.get_or_create("test123".to_string(), "https://example.com".to_string());

        assert_eq!(session.session_id, "test123");
        assert_eq!(session.origin_url, "https://example.com");
        assert_eq!(manager.session_count(), 1);
    }

    #[test]
    fn test_session_touch() {
        let manager = SessionManager::new(Duration::from_secs(300));
        let session =
            manager.get_or_create("test456".to_string(), "https://example.com".to_string());

        let initial_time = session.last_accessed;
        std::thread::sleep(Duration::from_millis(10));
        manager.touch("test456");

        let updated_session = manager.get("test456").unwrap();
        assert!(updated_session.last_accessed > initial_time);
    }

    #[test]
    fn test_session_removal() {
        let manager = SessionManager::new(Duration::from_secs(300));
        manager.get_or_create("test789".to_string(), "https://example.com".to_string());

        assert_eq!(manager.session_count(), 1);
        manager.remove("test789");
        assert_eq!(manager.session_count(), 0);
    }
}
