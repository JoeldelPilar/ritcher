//! Typed Axum extractors for input validation.
//!
//! Lifts validation from per-handler `validate_*` calls into the type system.
//! A handler that takes `ValidatedSessionId` or `ValidatedOrigin` cannot
//! receive an unvalidated value, because the wrapped fields are private and
//! the only way to construct one is via the `FromRequestParts` impls — which
//! run [`validate_session_id`] and [`validate_origin_url`] internally.
//!
//! ## Why types, not function calls
//!
//! Before this module, every handler had to remember to call
//! `validate_session_id(&id)?` and `validate_origin_url(origin)?` itself.
//! A new endpoint that forgot the call shipped an SSRF or character-set
//! bypass. Lifting validation to extractors makes the validation step a
//! declarative parameter — the compiler enforces nothing, but a code reviewer
//! can grep `git grep -E 'validate_origin_url|validate_session_id' src/server/handlers/`
//! and expect zero hits.
//!
//! ## Open TOCTOU window (out of scope)
//!
//! [`ValidatedOrigin`] currently stores only the parsed `Url`. The fetcher
//! resolves DNS again at request time, so a hostname that passed validation
//! could resolve to a private IP between check and fetch (DNS rebinding).
//! Closing this gap requires caching the resolved IP on the extractor and
//! forcing the fetcher to use IP + Host header. Tracked separately.

use crate::error::RitcherError;
use crate::server::url_validation::{validate_origin_url, validate_session_id};
use axum::extract::{FromRequestParts, Query, RawPathParams};
use axum::http::request::Parts;
use std::collections::HashMap;
use url::Url;

/// A session ID that has passed character-set and length validation.
///
/// Construct only via the [`FromRequestParts`] impl, which extracts the
/// first path parameter (`{session_id}` in route patterns) and runs
/// [`validate_session_id`]. The wrapped `String` is private; handlers
/// access it through [`ValidatedSessionId::as_str`] or
/// [`ValidatedSessionId::into_inner`].
///
/// Invalid input is rejected with HTTP 400 *before* the handler runs.
#[derive(Debug, Clone)]
pub struct ValidatedSessionId(String);

impl ValidatedSessionId {
    /// Borrow the validated session ID as `&str`.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Consume the wrapper and return the owned validated `String`.
    pub fn into_inner(self) -> String {
        self.0
    }
}

impl<S> FromRequestParts<S> for ValidatedSessionId
where
    S: Send + Sync,
{
    type Rejection = RitcherError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        // Use `RawPathParams` to read path parameters without colliding with
        // the handler's own `Path` extractor (axum's `Path` is documented as
        // not safe to use twice in one handler). `RawPathParams` is iterable
        // by reference and so can be re-read freely.
        let raw = RawPathParams::from_request_parts(parts, state)
            .await
            .map_err(|_| {
                RitcherError::InvalidSessionId("Missing session ID path parameter".to_string())
            })?;

        let session_id = raw
            .iter()
            .find(|(name, _)| *name == "session_id")
            .map(|(_, value)| value.to_string())
            .ok_or_else(|| {
                RitcherError::InvalidSessionId("Missing session ID path parameter".to_string())
            })?;

        validate_session_id(&session_id)?;
        Ok(ValidatedSessionId(session_id))
    }
}

/// An origin URL that has passed SSRF validation, or `None` if the
/// `?origin=` query parameter was absent.
///
/// Construct only via the [`FromRequestParts`] impl, which:
/// 1. Pulls the `origin` field out of the query string (if present).
/// 2. Runs [`validate_origin_url`] to reject private IPs, non-HTTP(S)
///    schemes, and IPv4-mapped/NAT64 bypass vectors.
/// 3. Parses the validated string into a [`Url`].
///
/// If the parameter is absent, the wrapper holds `None`. Handlers fall
/// back to `state.config.origin_url` (operator-trusted) in that case —
/// the config URL never flows through this extractor.
///
/// The wrapped `Url` is private; handlers access it through
/// [`ValidatedOrigin::as_url`], [`ValidatedOrigin::as_str`], or
/// [`ValidatedOrigin::into_inner`].
///
/// Invalid input is rejected with HTTP 400 *before* the handler runs.
#[derive(Debug, Clone)]
pub struct ValidatedOrigin(Option<Url>);

impl ValidatedOrigin {
    /// Borrow the validated origin URL if a `?origin=` parameter was provided.
    pub fn as_url(&self) -> Option<&Url> {
        self.0.as_ref()
    }

    /// Borrow the validated origin URL as a string slice if present.
    ///
    /// Returns the [`Url`]'s string form via [`Url::as_str`].
    pub fn as_str(&self) -> Option<&str> {
        self.0.as_ref().map(Url::as_str)
    }

    /// Consume the wrapper and return the owned [`Url`] (or `None`).
    pub fn into_inner(self) -> Option<Url> {
        self.0
    }
}

impl<S> FromRequestParts<S> for ValidatedOrigin
where
    S: Send + Sync,
{
    type Rejection = RitcherError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        // If the request has no query string at all, treat it as an absent
        // origin. Otherwise extract the `origin` field and validate it.
        // We deserialise into a HashMap so other query params (LL-HLS, dur,
        // track) still flow to handlers via their own Query<HashMap> extractor.
        let Ok(Query(params)) =
            Query::<HashMap<String, String>>::from_request_parts(parts, state).await
        else {
            return Ok(ValidatedOrigin(None));
        };

        let Some(raw) = params.get("origin") else {
            return Ok(ValidatedOrigin(None));
        };

        validate_origin_url(raw)?;
        let parsed = Url::parse(raw)
            .map_err(|_| RitcherError::InvalidOrigin(format!("Invalid URL: {raw}")))?;
        Ok(ValidatedOrigin(Some(parsed)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::Router;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use axum::routing::get;
    use tower::ServiceExt;

    // Minimal handlers that exercise just the extractor and return 200 +
    // a textual body so tests can assert on shape without setting up state.

    async fn echo_session(id: ValidatedSessionId) -> String {
        id.into_inner()
    }

    async fn echo_origin(origin: ValidatedOrigin) -> String {
        origin
            .as_str()
            .map(ToOwned::to_owned)
            .unwrap_or_else(|| "<none>".to_string())
    }

    fn session_app() -> Router {
        Router::new().route("/s/{session_id}", get(echo_session))
    }

    fn session_app_with_tuple() -> Router {
        // Mirrors real routes like `/stitch/{session_id}/segment/{*segment_path}`
        // — exercises the case where the handler also extracts via Path tuple.
        async fn echo_with_extra(
            id: ValidatedSessionId,
            axum::extract::Path((_id2, extra)): axum::extract::Path<(String, String)>,
        ) -> String {
            format!("{}-{}", id.into_inner(), extra)
        }
        Router::new().route("/s/{session_id}/x/{extra}", get(echo_with_extra))
    }

    fn origin_app() -> Router {
        Router::new().route("/o", get(echo_origin))
    }

    #[tokio::test]
    async fn valid_origin_extracts_ok() {
        let app = origin_app();
        let req = Request::builder()
            .uri("/o?origin=https%3A%2F%2Fcdn.example.com%2Flive.m3u8")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn missing_origin_param_yields_none() {
        let app = origin_app();
        let req = Request::builder().uri("/o").body(Body::empty()).unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        // Body should be the sentinel "<none>" — handler decides fallback.
        let body = axum::body::to_bytes(resp.into_body(), 64).await.unwrap();
        assert_eq!(&body[..], b"<none>");
    }

    #[tokio::test]
    async fn private_ip_origin_rejects_400() {
        let app = origin_app();
        let req = Request::builder()
            .uri("/o?origin=http%3A%2F%2F127.0.0.1%2Fstream")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn cloud_metadata_origin_rejects_400() {
        // 169.254.169.254 — the canonical SSRF target on AWS/GCP/Azure.
        let app = origin_app();
        let req = Request::builder()
            .uri("/o?origin=http%3A%2F%2F169.254.169.254%2Flatest")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn ftp_scheme_origin_rejects_400() {
        let app = origin_app();
        let req = Request::builder()
            .uri("/o?origin=ftp%3A%2F%2Fcdn.example.com%2Ffile.ts")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn valid_session_id_extracts_ok() {
        let app = session_app();
        let req = Request::builder()
            .uri("/s/abc-123_XYZ")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn invalid_session_id_chars_reject_400() {
        let app = session_app();
        // A dot is not in the allowed character class.
        let req = Request::builder()
            .uri("/s/bad.session")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn session_id_too_long_rejects_400() {
        let long_id = "a".repeat(65);
        let app = session_app();
        let req = Request::builder()
            .uri(format!("/s/{}", long_id))
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn extractor_coexists_with_path_tuple() {
        // Real handlers like `/stitch/{session_id}/segment/{*segment_path}`
        // extract a `Path<(String, String)>` for both params. The
        // `ValidatedSessionId` extractor must coexist with that without
        // colliding with axum's `Path` machinery.
        let app = session_app_with_tuple();
        let req = Request::builder()
            .uri("/s/sess-1/x/seg42")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), 64).await.unwrap();
        assert_eq!(&body[..], b"sess-1-seg42");
    }
}
