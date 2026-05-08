//! Session storage abstraction.
//!
//! [`SessionStore`] is the seam between [`SessionManager`](super::SessionManager)
//! and the concrete persistence backend (in-process [`MemoryStore`](super::memory::MemoryStore)
//! or [`ValkeyStore`](super::valkey::ValkeyStore)).
//!
//! ## Design notes
//!
//! - The TTL is owned by [`SessionManager`] and passed into store calls that
//!   need it (`insert_if_absent`, `touch`, `cleanup_expired`). This keeps the
//!   store implementations stateless w.r.t. TTL configuration and lets a single
//!   manager swap backends without rewiring lifetime knobs.
//! - `session_count` is intentionally an *approximation* on cluster-style
//!   backends. The Valkey implementation walks keys with `SCAN` to avoid
//!   blocking the server with `KEYS`; the count may drift while iterating
//!   and is meant for diagnostics, not for eviction logic.
//! - `cleanup_expired` is a no-op for backends with native TTL (Valkey).
//!   Memory implementations must enforce TTL themselves.
//!
//! All store errors are surfaced as [`SessionError`]; the manager logs and
//! degrades gracefully (e.g. returning `None` for a failed `get`).
//!
//! Tests construct a `FakeSessionStore` directly against this trait — see
//! the `tests` module in [`super::manager`].

use super::Session;
use async_trait::async_trait;
use std::time::{Duration, SystemTime};

/// Errors surfaced by a [`SessionStore`] implementation.
///
/// The manager layer translates these into log lines and `Option::None` for
/// callers; handlers never see a `SessionError` directly.
#[derive(Debug, thiserror::Error)]
pub enum SessionError {
    /// Underlying backend (network, codec, parse) failure.
    ///
    /// Carries a human-readable cause for `tracing` only — never bubbled to
    /// HTTP clients verbatim (see [`crate::error::RitcherError`]).
    #[error("session backend error: {0}")]
    Backend(String),
}

/// Storage seam for session persistence.
///
/// Implementations:
/// - [`super::memory::MemoryStore`] — `DashMap`-backed, in-process, default.
/// - [`super::valkey::ValkeyStore`] — Redis/Valkey-backed, distributed
///   (gated behind the `valkey` cargo feature).
///
/// All methods are `async` so the same trait fits both lock-free in-memory
/// stores and network-backed stores. The trait is `Send + Sync` so it can
/// live behind `Arc<dyn SessionStore>` inside a cloneable
/// [`SessionManager`](super::SessionManager).
#[async_trait]
pub trait SessionStore: Send + Sync {
    /// Fetch a session by ID. Returns `Ok(None)` if not present.
    async fn get(&self, session_id: &str) -> Result<Option<Session>, SessionError>;

    /// Insert a session if no entry exists for the given ID; otherwise
    /// return the existing record unchanged.
    ///
    /// `ttl` is forwarded to backends that need it for native expiry
    /// (e.g. Valkey `SET ... EX <ttl_secs>`). In-memory backends may
    /// ignore it and rely on `cleanup_expired`.
    ///
    /// The returned [`Session`] is the one that ultimately won — either
    /// `session` itself (on insert) or the existing record (on hit).
    ///
    /// Implementations may resolve concurrent racers via last-writer-wins;
    /// callers must not rely on strict CAS semantics. See the per-impl docs
    /// for atomicity guarantees.
    async fn insert_if_absent(
        &self,
        session: Session,
        ttl: Duration,
    ) -> Result<Session, SessionError>;

    /// Refresh the access time / TTL for `session_id`.
    ///
    /// Memory implementations update `Session::last_accessed`. Valkey
    /// implementations issue `EXPIRE` to refresh native TTL without
    /// rewriting the stored JSON — this is an intentional trade-off
    /// documented on [`super::SessionManager::touch`].
    async fn touch(&self, session_id: &str, ttl: Duration) -> Result<(), SessionError>;

    /// Remove a session by ID. Returns the removed record if it existed.
    async fn remove(&self, session_id: &str) -> Result<Option<Session>, SessionError>;

    /// Best-effort eviction of sessions whose `last_accessed + ttl < now`.
    ///
    /// Backends with native TTL (Valkey) implement this as a no-op.
    async fn cleanup_expired(&self, ttl: Duration, now: SystemTime) -> Result<(), SessionError>;

    /// Approximate count of live sessions.
    ///
    /// Exact for in-memory backends. On Valkey this is a `SCAN`-based
    /// approximation: concurrent inserts/expirations during the walk may
    /// be missed or double-counted, and only `ritcher:session:*` keys
    /// are observed. Use for diagnostics, not for correctness.
    async fn approx_count(&self) -> Result<usize, SessionError>;
}
