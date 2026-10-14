//! Authentication: password hashing, opaque device tokens, request extractors
//! and the in-memory login rate limiter.
//!
//! Tokens are 32 bytes of OS randomness, base64url-encoded without padding. Only
//! `sha256(token)` (hex) is ever persisted; the raw token is returned once at
//! login and is never logged.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use argon2::password_hash::rand_core::OsRng;
use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::Argon2;
use axum::extract::{FromRequestParts, Request, State};
use axum::http::header::AUTHORIZATION;
use axum::http::request::Parts;
use axum::middleware::Next;
use axum::response::Response;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use flex_protocol::{User, UserRole};
use rand::RngCore;
use sha2::{Digest, Sha256};
use sqlx::Row;

use crate::api::AppState;
use crate::db;
use crate::error::AppError;

/// Lifetime of a freshly issued device token (90 days).
pub const TOKEN_TTL_SECS: i64 = 90 * 24 * 60 * 60;

/// Minimum interval between `last_seen_at` writes for a device.
pub const LAST_SEEN_THROTTLE_SECS: i64 = 60;

/// Failed logins allowed within [`LOGIN_WINDOW`] before lockout.
pub const LOGIN_MAX_FAILURES: usize = 5;

/// Sliding window for login rate limiting.
pub const LOGIN_WINDOW: Duration = Duration::from_secs(15 * 60);

/// Hash a plaintext password into a PHC string using Argon2id (default params).
pub fn hash_password(password: &str) -> Result<String, argon2::password_hash::Error> {
    let salt = SaltString::generate(&mut OsRng);
    let hash = Argon2::default().hash_password(password.as_bytes(), &salt)?;
    Ok(hash.to_string())
}

/// Verify a password against a stored PHC string. Returns `false` on any error,
/// including a malformed stored hash, so callers cannot learn why it failed.
pub fn verify_password(password: &str, phc: &str) -> bool {
    match PasswordHash::new(phc) {
        Ok(parsed) => Argon2::default()
            .verify_password(password.as_bytes(), &parsed)
            .is_ok(),
        Err(_) => false,
    }
}

/// Generate a new opaque bearer token (32 random bytes, base64url, no padding).
pub fn generate_token() -> String {
    let mut bytes = [0u8; 32];
    OsRng.fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

/// Hash a token for storage/lookup: hex-encoded SHA-256.
pub fn hash_token(token: &str) -> String {
    let digest = Sha256::digest(token.as_bytes());
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in digest {
        use std::fmt::Write as _;
        let _ = write!(out, "{byte:02x}");
    }
    out
}

/// The authenticated caller, produced by [`AuthUser::from_request_parts`].
#[derive(Debug, Clone)]
pub struct AuthUser {
    pub user: User,
    /// Row id of the device token used for the request.
    pub device_id: i64,
}

impl AuthUser {
    /// Whether the authenticated user is an administrator.
    pub fn is_admin(&self) -> bool {
        self.user.role == UserRole::Admin
    }
}

fn bearer_token(parts: &Parts) -> Option<String> {
    let value = parts.headers.get(AUTHORIZATION)?.to_str().ok()?;
    let (scheme, token) = value.split_once(' ')?;
    if !scheme.eq_ignore_ascii_case("bearer") {
        return None;
    }
    let token = token.trim();
    if token.is_empty() {
        None
    } else {
        Some(token.to_string())
    }
}

impl FromRequestParts<AppState> for AuthUser {
    type Rejection = AppError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        // The auth middleware resolves the token once and stores the result;
        // handlers then read it without a second database round-trip.
        if let Some(existing) = parts.extensions.get::<AuthUser>() {
            return Ok(existing.clone());
        }

        let token =
            bearer_token(parts).ok_or_else(|| AppError::unauthorized("missing bearer token"))?;
        let token_hash = hash_token(&token);

        let row = sqlx::query(
            "SELECT d.id AS device_id, d.expires_at, d.last_seen_at, \
                    u.id AS user_id, u.username, u.role, u.created_at \
             FROM devices d JOIN users u ON u.id = d.user_id \
             WHERE d.token_hash = ?",
        )
        .bind(&token_hash)
        .fetch_optional(&state.pool)
        .await?;

        let row = row.ok_or_else(|| AppError::unauthorized("invalid or expired token"))?;

        let now = db::now_secs();
        let device_id: i64 = row.try_get("device_id")?;
        let expires_at: Option<i64> = row.try_get("expires_at")?;
        if expires_at.is_some_and(|exp| exp <= now) {
            // Reclaim the expired row opportunistically.
            let _ = sqlx::query("DELETE FROM devices WHERE id = ?")
                .bind(device_id)
                .execute(&state.pool)
                .await;
            return Err(AppError::unauthorized("token expired"));
        }

        let role: String = row.try_get("role")?;
        let user = User {
            id: row.try_get("user_id")?,
            username: row.try_get("username")?,
            role: crate::model::role_from_str(&role),
            created_at: row.try_get("created_at")?,
        };

        let last_seen: Option<i64> = row.try_get("last_seen_at")?;
        if last_seen.is_none_or(|seen| now - seen >= LAST_SEEN_THROTTLE_SECS) {
            let _ = sqlx::query("UPDATE devices SET last_seen_at = ? WHERE id = ?")
                .bind(now)
                .bind(device_id)
                .execute(&state.pool)
                .await;
        }

        Ok(AuthUser { user, device_id })
    }
}

/// Extractor that additionally requires the [`UserRole::Admin`] role.
#[derive(Debug, Clone, Copy)]
pub struct AdminUser;

impl FromRequestParts<AppState> for AdminUser {
    type Rejection = AppError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let auth = AuthUser::from_request_parts(parts, state).await?;
        if auth.is_admin() {
            Ok(AdminUser)
        } else {
            Err(AppError::forbidden("administrator privileges required"))
        }
    }
}

/// Middleware applying [`AuthUser`] to a whole router.
///
/// On success the resolved [`AuthUser`] is inserted into the request extensions
/// so downstream handlers avoid re-querying the database.
pub async fn auth_middleware(
    State(state): State<AppState>,
    request: Request,
    next: Next,
) -> Result<Response, AppError> {
    let (mut parts, body) = request.into_parts();
    let auth = AuthUser::from_request_parts(&mut parts, &state).await?;
    parts.extensions.insert(auth);
    Ok(next.run(Request::from_parts(parts, body)).await)
}

/// Key for the login rate limiter: username plus client IP.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RateKey {
    pub username: String,
    pub ip: String,
}

/// Bounded in-memory limiter keyed on `(username, client IP)`.
///
/// A key is locked once it accumulates [`LOGIN_MAX_FAILURES`] failures within a
/// sliding [`LOGIN_WINDOW`]; a successful login clears the key.
#[derive(Debug, Default)]
pub struct LoginRateLimiter {
    entries: Mutex<HashMap<RateKey, Vec<Instant>>>,
}

impl LoginRateLimiter {
    /// Create an empty limiter.
    pub fn new() -> Self {
        Self::default()
    }

    fn prune(entry: &mut Vec<Instant>, now: Instant) {
        entry.retain(|at| now.duration_since(*at) < LOGIN_WINDOW);
    }

    /// Whether the key currently has too many recent failures.
    pub fn is_locked(&self, key: &RateKey) -> bool {
        let now = Instant::now();
        let mut entries = self.entries.lock().expect("rate limiter mutex poisoned");
        match entries.get_mut(key) {
            Some(entry) => {
                Self::prune(entry, now);
                if entry.is_empty() {
                    entries.remove(key);
                    false
                } else {
                    entry.len() >= LOGIN_MAX_FAILURES
                }
            }
            None => false,
        }
    }

    /// Record a failed login for the key.
    pub fn record_failure(&self, key: RateKey) {
        let now = Instant::now();
        let mut entries = self.entries.lock().expect("rate limiter mutex poisoned");
        if entries.len() > 4096 {
            entries.retain(|_, entry| {
                Self::prune(entry, now);
                !entry.is_empty()
            });
        }
        let entry = entries.entry(key).or_default();
        entry.push(now);
        Self::prune(entry, now);
    }

    /// Clear the key after a successful login.
    pub fn record_success(&self, key: &RateKey) {
        let mut entries = self.entries.lock().expect("rate limiter mutex poisoned");
        entries.remove(key);
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::config::Config;
    use axum::http::Request;
    use flex_protocol::UserRole;

    fn parts_with_auth(token: Option<&str>) -> Parts {
        let mut builder = Request::builder().uri("/");
        if let Some(token) = token {
            builder = builder.header(AUTHORIZATION, format!("Bearer {token}"));
        }
        builder.body(()).unwrap().into_parts().0
    }

    async fn test_state() -> AppState {
        AppState {
            pool: db::test_pool().await,
            config: Arc::new(Config::default()),
            rate_limiter: Arc::new(LoginRateLimiter::new()),
            sessions: Arc::new(crate::playback::SessionManager::new(
                std::env::temp_dir(),
                2,
            )),
            transcode: Arc::new(crate::transcode::TranscodeCapabilities::default()),
        }
    }

    async fn seed_user(state: &AppState, username: &str, role: UserRole) -> User {
        let hash = hash_password("pw").unwrap();
        db::create_user(&state.pool, username, &hash, role)
            .await
            .unwrap()
    }

    #[test]
    fn password_hash_roundtrip() {
        let hash = hash_password("hunter2").unwrap();
        assert!(hash.starts_with("$argon2id$"));
        assert!(verify_password("hunter2", &hash));
        assert!(!verify_password("hunter3", &hash));
        assert!(!verify_password("hunter2", "not-a-phc-string"));
    }

    #[test]
    fn token_generate_hash_verify() {
        let token = generate_token();
        let decoded = URL_SAFE_NO_PAD.decode(token.as_bytes()).unwrap();
        assert_eq!(decoded.len(), 32);

        let hash = hash_token(&token);
        assert_eq!(hash.len(), 64);
        assert_eq!(hash, hash_token(&token));
        assert_ne!(hash, hash_token(&generate_token()));
    }

    #[test]
    fn rate_limiter_locks_after_repeated_failures() {
        let limiter = LoginRateLimiter::new();
        let key = RateKey {
            username: "alice".into(),
            ip: "127.0.0.1".into(),
        };
        for _ in 0..LOGIN_MAX_FAILURES {
            assert!(!limiter.is_locked(&key));
            limiter.record_failure(key.clone());
        }
        assert!(limiter.is_locked(&key));

        // A different network peer is unaffected.
        let other = RateKey {
            username: "alice".into(),
            ip: "10.0.0.1".into(),
        };
        assert!(!limiter.is_locked(&other));

        limiter.record_success(&key);
        assert!(!limiter.is_locked(&key));
    }

    #[tokio::test]
    async fn rejects_missing_token() {
        let state = test_state().await;
        let mut parts = parts_with_auth(None);
        let result = AuthUser::from_request_parts(&mut parts, &state).await;
        assert!(matches!(result, Err(AppError::Unauthorized(_))));
    }

    #[tokio::test]
    async fn rejects_unknown_token() {
        let state = test_state().await;
        let mut parts = parts_with_auth(Some("totally-unknown-token"));
        let result = AuthUser::from_request_parts(&mut parts, &state).await;
        assert!(matches!(result, Err(AppError::Unauthorized(_))));
    }

    #[tokio::test]
    async fn rejects_expired_token() {
        let state = test_state().await;
        let user = seed_user(&state, "alice", UserRole::User).await;
        let token = generate_token();
        db::create_device(
            &state.pool,
            user.id,
            "old phone",
            &hash_token(&token),
            db::now_secs() - 100,
            db::now_secs() - 1,
        )
        .await
        .unwrap();

        let mut parts = parts_with_auth(Some(&token));
        let result = AuthUser::from_request_parts(&mut parts, &state).await;
        assert!(matches!(result, Err(AppError::Unauthorized(_))));
    }

    #[tokio::test]
    async fn accepts_valid_token_and_yields_user() {
        let state = test_state().await;
        let user = seed_user(&state, "alice", UserRole::Admin).await;
        let token = generate_token();
        let device_id = db::create_device(
            &state.pool,
            user.id,
            "laptop",
            &hash_token(&token),
            db::now_secs(),
            db::now_secs() + TOKEN_TTL_SECS,
        )
        .await
        .unwrap();

        let mut parts = parts_with_auth(Some(&token));
        let auth = AuthUser::from_request_parts(&mut parts, &state)
            .await
            .unwrap();
        assert_eq!(auth.user.username, "alice");
        assert_eq!(auth.device_id, device_id);
        assert!(auth.is_admin());
    }

    #[tokio::test]
    async fn admin_extractor_rejects_plain_user() {
        let state = test_state().await;
        let user = seed_user(&state, "bob", UserRole::User).await;
        let token = generate_token();
        db::create_device(
            &state.pool,
            user.id,
            "phone",
            &hash_token(&token),
            db::now_secs(),
            db::now_secs() + TOKEN_TTL_SECS,
        )
        .await
        .unwrap();

        let mut parts = parts_with_auth(Some(&token));
        let result = AdminUser::from_request_parts(&mut parts, &state).await;
        assert!(matches!(result, Err(AppError::Forbidden(_))));
    }
}
