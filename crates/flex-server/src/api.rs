//! HTTP API: routes, handlers and byte-range streaming.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use axum::body::Body;
use axum::extract::{ConnectInfo, Path, Query, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::middleware;
use axum::response::Response;
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use flex_protocol::{
    ContinueWatchingEntry, CreateUserRequest, Device, Health, ItemQuery, Library, LoginRequest,
    LoginResponse, MediaItem, Page, ScanReport, SubtitleStream, UpdateProgressRequest, User,
    UserRole, WatchState, API_VERSION,
};
use serde::Deserialize;
use sqlx::SqlitePool;
use tokio::io::{AsyncReadExt, AsyncSeekExt, SeekFrom};
use tokio_util::io::ReaderStream;
use tower_http::cors::CorsLayer;
use tower_http::services::{ServeDir, ServeFile};
use tower_http::trace::TraceLayer;

use crate::auth::{self, AdminUser, AuthUser, LoginRateLimiter, RateKey};
use crate::config::Config;
use crate::error::AppError;
use crate::playback::{self, SessionManager, StreamAccess};
use crate::subtitles::SubtitleRegistry;
use crate::transcode::TranscodeCapabilities;
use crate::{db, scanner, subtitles};

/// Shared application state.
#[derive(Clone)]
pub struct AppState {
    pub pool: SqlitePool,
    pub config: Arc<Config>,
    /// In-memory per-`(username, IP)` login rate limiter.
    pub rate_limiter: Arc<LoginRateLimiter>,
    /// Live playback sessions (direct, remux, transcode).
    pub sessions: Arc<SessionManager>,
    /// Probed `ffmpeg` capability set.
    pub transcode: Arc<TranscodeCapabilities>,
    /// Configured online subtitle providers.
    pub subtitles: Arc<SubtitleRegistry>,
}

/// Build the application router.
///
/// Everything under `/api/v1` requires a bearer token *except* the health probe,
/// login, the HLS playback routes (authorised by a scoped `?t=` token) and the
/// byte-range stream route (which accepts either a bearer token or a scoped
/// direct-play token). Those live outside the [`crate::auth::auth_middleware`]
/// layer; see [`protected_router`].
pub fn router(state: AppState) -> Router {
    let protected = protected_router(state.clone());

    let mut router = Router::new()
        .route("/api/v1/health", get(health))
        .route("/api/v1/auth/login", post(login))
        // Scoped playback routes: authorised by `?t=<session_token>` rather than
        // a bearer token, because HLS players cannot attach custom headers.
        .route(
            "/api/v1/playback/{session_id}/master.m3u8",
            get(playback::master),
        )
        .route(
            "/api/v1/playback/{session_id}/{file}",
            get(playback::segment),
        )
        // Direct play uses a Range-capable file route that accepts either a
        // bearer token or a scoped direct-play session token.
        .route("/api/v1/items/{id}/stream", get(stream))
        .merge(protected);

    // Static web UI: unauthenticated fallback so unknown non-API paths return
    // the SPA shell. Registered routes (including all of `/api/*`) win over the
    // fallback because `fallback_service` only runs when nothing else matches.
    match state.config.web_path() {
        Some(web_dir) => {
            let index = web_dir.join("index.html");
            tracing::info!(dir = %web_dir.display(), "serving static web UI");
            router =
                router.fallback_service(ServeDir::new(web_dir).fallback(ServeFile::new(index)));
        }
        None => tracing::info!("no web/ directory found; static web UI disabled"),
    }

    router
        .layer(CorsLayer::permissive())
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

/// Sub-router carrying every bearer-token-protected route.
///
/// The auth layer is applied via `route_layer` so it only runs for routes in
/// this router; new protected handlers can simply be `.route`d onto it.
pub fn protected_router(state: AppState) -> Router<AppState> {
    Router::new()
        .route("/api/v1/libraries", get(list_libraries))
        .route("/api/v1/scan", post(trigger_scan))
        .route("/api/v1/items", get(list_items))
        .route("/api/v1/items/{id}", get(get_item))
        .route("/api/v1/items/{id}/artwork", get(artwork))
        .route("/api/v1/items/{id}/subtitles", get(list_subtitles))
        .route(
            "/api/v1/items/{id}/subtitles/{track}",
            get(get_subtitle_track),
        )
        .route(
            flex_protocol::subtitle_search_path().as_str(),
            get(subtitles::search),
        )
        .route(
            flex_protocol::subtitle_download_path().as_str(),
            post(subtitles::download),
        )
        .route("/api/v1/playback/start", post(playback::start))
        .route("/api/v1/playback/stop", post(playback::stop))
        .route("/api/v1/system/info", get(playback::system_info))
        .route("/api/v1/continue-watching", get(continue_watching))
        .route("/api/v1/progress", get(list_progress))
        .route(
            "/api/v1/progress/{item_id}",
            get(get_progress).put(put_progress).delete(delete_progress),
        )
        .route("/api/v1/auth/logout", post(logout))
        .route("/api/v1/auth/me", get(me))
        .route("/api/v1/auth/devices", get(list_devices))
        .route("/api/v1/auth/devices/{id}", delete(delete_device))
        .route("/api/v1/auth/users", get(list_users).post(create_user))
        .route_layer(middleware::from_fn_with_state(state, auth::auth_middleware))
}

async fn health() -> Json<Health> {
    Json(Health {
        status: "ok".to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        api_version: API_VERSION.to_string(),
    })
}

/// Exchange credentials for a device token.
async fn login(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    Json(request): Json<LoginRequest>,
) -> Result<Json<LoginResponse>, AppError> {
    let username = request.username.trim();
    let key = RateKey {
        username: username.to_string(),
        ip: peer.ip().to_string(),
    };

    if state.rate_limiter.is_locked(&key) {
        return Err(AppError::too_many_requests(
            "too many failed login attempts; try again later",
        ));
    }

    let found = db::find_user_by_username(&state.pool, username).await?;
    let valid = found
        .as_ref()
        .map(|(_, hash)| auth::verify_password(&request.password, hash))
        .unwrap_or(false);
    if !valid {
        state.rate_limiter.record_failure(key);
        return Err(AppError::unauthorized("invalid username or password"));
    }

    let (user, _) = found.expect("valid credentials imply a user row");
    state.rate_limiter.record_success(&key);

    let token = auth::generate_token();
    let token_hash = auth::hash_token(&token);
    let now = db::now_secs();
    let expires_at = now + auth::TOKEN_TTL_SECS;
    let device_name = request
        .device_name
        .as_deref()
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .unwrap_or("Unknown device");
    db::create_device(
        &state.pool,
        user.id,
        device_name,
        &token_hash,
        now,
        expires_at,
    )
    .await?;

    Ok(Json(LoginResponse {
        token,
        user,
        expires_at: Some(expires_at),
    }))
}

/// Revoke the token used for the request.
async fn logout(auth: AuthUser, State(state): State<AppState>) -> Result<StatusCode, AppError> {
    db::delete_device(&state.pool, auth.user.id, auth.device_id).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Return the authenticated user.
async fn me(auth: AuthUser) -> Json<User> {
    Json(auth.user)
}

/// List the authenticated user's device tokens.
async fn list_devices(
    auth: AuthUser,
    State(state): State<AppState>,
) -> Result<Json<Vec<Device>>, AppError> {
    let mut devices = db::list_devices(&state.pool, auth.user.id).await?;
    for device in &mut devices {
        device.current = device.id == auth.device_id;
    }
    Ok(Json(devices))
}

/// Revoke one of the authenticated user's other device tokens.
async fn delete_device(
    auth: AuthUser,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<StatusCode, AppError> {
    if db::delete_device(&state.pool, auth.user.id, id).await? {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(AppError::not_found(format!("device {id} not found")))
    }
}

/// List every user (admin only).
async fn list_users(
    _admin: AdminUser,
    State(state): State<AppState>,
) -> Result<Json<Vec<User>>, AppError> {
    Ok(Json(db::list_users(&state.pool).await?))
}

/// Create a user (admin only).
async fn create_user(
    _admin: AdminUser,
    State(state): State<AppState>,
    Json(request): Json<CreateUserRequest>,
) -> Result<Json<User>, AppError> {
    let username = request.username.trim();
    if username.is_empty() {
        return Err(AppError::bad_request("username must not be empty"));
    }
    if request.password.is_empty() {
        return Err(AppError::bad_request("password must not be empty"));
    }
    if db::find_user_by_username(&state.pool, username)
        .await?
        .is_some()
    {
        return Err(AppError::conflict(format!(
            "user {username:?} already exists"
        )));
    }

    let hash = auth::hash_password(&request.password)
        .map_err(|e| AppError::internal(format!("password hashing failed: {e}")))?;
    let role = if request.admin {
        UserRole::Admin
    } else {
        UserRole::User
    };
    let user = db::create_user(&state.pool, username, &hash, role).await?;
    Ok(Json(user))
}

async fn list_libraries(State(state): State<AppState>) -> Result<Json<Vec<Library>>, AppError> {
    let libraries = db::list_libraries(&state.pool).await?;
    Ok(Json(libraries))
}

async fn trigger_scan(State(state): State<AppState>) -> Result<Json<ScanReport>, AppError> {
    let report = scanner::scan(&state.pool, &state.config).await;
    Ok(Json(report))
}

async fn list_items(
    State(state): State<AppState>,
    Query(query): Query<ItemQuery>,
) -> Result<Json<Page<MediaItem>>, AppError> {
    let page = db::list_items(&state.pool, &query).await?;
    Ok(Json(page))
}

async fn get_item(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Json<MediaItem>, AppError> {
    db::get_item(&state.pool, id)
        .await?
        .map(Json)
        .ok_or_else(|| AppError::not_found(format!("media item {id} not found")))
}

/// Pagination query for `GET /continue-watching`.
#[derive(Debug, Deserialize)]
struct ProgressQuery {
    limit: Option<i64>,
    offset: Option<i64>,
}

/// List the user's resumable items, most recently watched first.
async fn continue_watching(
    auth: AuthUser,
    State(state): State<AppState>,
    Query(query): Query<ProgressQuery>,
) -> Result<Json<Page<ContinueWatchingEntry>>, AppError> {
    let limit = query.limit.unwrap_or(20).clamp(1, 100);
    let offset = query.offset.unwrap_or(0).max(0);
    let page = db::list_continue_watching(&state.pool, auth.user.id, limit, offset).await?;
    Ok(Json(page))
}

/// List every watch-state row for the user.
async fn list_progress(
    auth: AuthUser,
    State(state): State<AppState>,
) -> Result<Json<Vec<WatchState>>, AppError> {
    Ok(Json(
        db::list_watch_states(&state.pool, auth.user.id).await?,
    ))
}

/// Fetch the user's watch state for one item.
async fn get_progress(
    auth: AuthUser,
    State(state): State<AppState>,
    Path(item_id): Path<i64>,
) -> Result<Json<WatchState>, AppError> {
    db::get_watch_state(&state.pool, auth.user.id, item_id)
        .await?
        .map(Json)
        .ok_or_else(|| AppError::not_found(format!("no progress recorded for item {item_id}")))
}

/// Upsert the user's watch state for one item.
///
/// `watched` is honoured when supplied; otherwise it is inferred as `true` once
/// the position reaches 90% of a known duration. The position is clamped to the
/// duration when one is known.
async fn put_progress(
    auth: AuthUser,
    State(state): State<AppState>,
    Path(item_id): Path<i64>,
    Json(request): Json<UpdateProgressRequest>,
) -> Result<Json<WatchState>, AppError> {
    if db::get_item(&state.pool, item_id).await?.is_none() {
        return Err(AppError::not_found(format!(
            "media item {item_id} not found"
        )));
    }

    let existing = db::get_watch_state(&state.pool, auth.user.id, item_id).await?;
    let duration = request
        .duration_ms
        .or_else(|| existing.as_ref().and_then(|state| state.duration_ms));
    let position = db::clamp_position_ms(request.position_ms, duration);
    let watched = request
        .watched
        .unwrap_or_else(|| db::infer_watched(position, duration));

    let saved = db::upsert_watch_state(
        &state.pool,
        auth.user.id,
        item_id,
        position,
        duration,
        watched,
        db::now_secs(),
    )
    .await?;
    Ok(Json(saved))
}

/// Clear the user's watch state for one item.
///
/// Idempotent: returns `204` whether or not a row existed.
async fn delete_progress(
    auth: AuthUser,
    State(state): State<AppState>,
    Path(item_id): Path<i64>,
) -> Result<StatusCode, AppError> {
    db::delete_watch_state(&state.pool, auth.user.id, item_id).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Parsed byte-range request.
#[derive(Debug, PartialEq, Eq)]
pub struct ByteRange {
    pub start: u64,
    pub end: u64,
}

/// Why a `Range` header could not be honoured.
#[derive(Debug, PartialEq, Eq)]
pub enum RangeError {
    Malformed,
    Unsatisfiable,
}

/// Parse a single-range `Range` header against a known resource size.
pub fn parse_range(header: &str, size: u64) -> Result<ByteRange, RangeError> {
    let spec = header.trim();
    let spec = spec.strip_prefix("bytes=").ok_or(RangeError::Malformed)?;
    let first = spec.split(',').next().unwrap_or("").trim();
    let (start_str, end_str) = first.split_once('-').ok_or(RangeError::Malformed)?;

    if size == 0 {
        return Err(RangeError::Unsatisfiable);
    }

    if start_str.is_empty() {
        // Suffix range: last N bytes.
        let suffix: u64 = end_str.parse().map_err(|_| RangeError::Malformed)?;
        if suffix == 0 {
            return Err(RangeError::Unsatisfiable);
        }
        let start = size.saturating_sub(suffix);
        return Ok(ByteRange {
            start,
            end: size - 1,
        });
    }

    let start: u64 = start_str.parse().map_err(|_| RangeError::Malformed)?;
    if start >= size {
        return Err(RangeError::Unsatisfiable);
    }
    let end = if end_str.is_empty() {
        size - 1
    } else {
        let requested: u64 = end_str.parse().map_err(|_| RangeError::Malformed)?;
        requested.min(size - 1)
    };
    if end < start {
        return Err(RangeError::Malformed);
    }
    Ok(ByteRange { start, end })
}

/// Guess a video MIME type from the container name reported by `ffprobe`.
pub fn content_type_for(container: Option<&str>) -> &'static str {
    let c = container.unwrap_or("").to_ascii_lowercase();
    let has = |needle: &str| c.contains(needle);
    if has("matroska") {
        "video/x-matroska"
    } else if has("webm") {
        "video/webm"
    } else if has("mp4") || has("m4v") {
        "video/mp4"
    } else if has("quicktime") || has("mov") {
        "video/quicktime"
    } else if has("avi") {
        "video/x-msvideo"
    } else if has("mpegts") {
        "video/mp2t"
    } else {
        "application/octet-stream"
    }
}

/// Guess a video MIME type using the file extension when available, then the
/// container name reported by `ffprobe`. MOV and MP4 share ffprobe format
/// aliases, so the extension is needed to distinguish their MIME types.
pub fn content_type_for_path(path: &str, container: Option<&str>) -> &'static str {
    let extension = PathBuf::from(path)
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    match extension.as_str() {
        "mov" => "video/quicktime",
        "mp4" | "m4v" => "video/mp4",
        _ => content_type_for(container),
    }
}

fn image_content_type(path: &str) -> &'static str {
    match path.rsplit('.').next().map(|e| e.to_ascii_lowercase()) {
        Some(ext) if ext == "png" => "image/png",
        Some(ext) if ext == "webp" => "image/webp",
        Some(ext) if ext == "gif" => "image/gif",
        _ => "image/jpeg",
    }
}

/// Open a file and build a streaming body, optionally limited to `len` bytes.
pub(crate) async fn file_body(
    path: &std::path::Path,
    start: u64,
    len: Option<u64>,
) -> Result<Body, AppError> {
    let mut file = tokio::fs::File::open(path).await?;
    if start > 0 {
        file.seek(SeekFrom::Start(start)).await?;
    }
    match len {
        Some(len) => Ok(Body::from_stream(ReaderStream::new(file.take(len)))),
        None => Ok(Body::from_stream(ReaderStream::new(file))),
    }
}

async fn stream(
    access: StreamAccess,
    State(state): State<AppState>,
    Path(id): Path<i64>,
    headers: HeaderMap,
) -> Result<Response, AppError> {
    if let Some(session_item) = access.via_session_item {
        if session_item != id {
            return Err(AppError::forbidden(
                "session token is not valid for this item",
            ));
        }
    }

    let item = db::get_item(&state.pool, id)
        .await?
        .ok_or_else(|| AppError::not_found(format!("media item {id} not found")))?;

    let path = PathBuf::from(&item.file_path);
    let metadata = tokio::fs::metadata(&path)
        .await
        .map_err(|_| AppError::not_found(format!("file for item {id} is unavailable")))?;
    let size = metadata.len();
    let content_type = content_type_for_path(&item.file_path, item.container.as_deref());

    let range_header = headers
        .get(header::RANGE)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());

    match range_header {
        Some(raw) => match parse_range(&raw, size) {
            Ok(range) => {
                let len = range.end - range.start + 1;
                let body = file_body(&path, range.start, Some(len)).await?;
                let response = Response::builder()
                    .status(StatusCode::PARTIAL_CONTENT)
                    .header(header::CONTENT_TYPE, content_type)
                    .header(header::ACCEPT_RANGES, "bytes")
                    .header(
                        header::CONTENT_RANGE,
                        format!("bytes {}-{}/{}", range.start, range.end, size),
                    )
                    .header(header::CONTENT_LENGTH, len)
                    .body(body)
                    .map_err(|e| AppError::internal(e.to_string()))?;
                Ok(response)
            }
            Err(RangeError::Unsatisfiable) => {
                let response = Response::builder()
                    .status(StatusCode::RANGE_NOT_SATISFIABLE)
                    .header(header::CONTENT_RANGE, format!("bytes */{size}"))
                    .header(header::ACCEPT_RANGES, "bytes")
                    .body(Body::empty())
                    .map_err(|e| AppError::internal(e.to_string()))?;
                Ok(response)
            }
            Err(RangeError::Malformed) => Err(AppError::bad_request("malformed Range header")),
        },
        None => {
            let body = file_body(&path, 0, None).await?;
            let response = Response::builder()
                .status(StatusCode::OK)
                .header(header::CONTENT_TYPE, content_type)
                .header(header::ACCEPT_RANGES, "bytes")
                .header(header::CONTENT_LENGTH, size)
                .body(body)
                .map_err(|e| AppError::internal(e.to_string()))?;
            Ok(response)
        }
    }
}

#[derive(Debug, Deserialize)]
struct ArtworkQuery {
    #[serde(rename = "type", default)]
    kind: Option<String>,
}

async fn artwork(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Query(query): Query<ArtworkQuery>,
) -> Result<Response, AppError> {
    let kind = query.kind.as_deref().unwrap_or("poster");
    let (poster, backdrop) = db::get_artwork_paths(&state.pool, id)
        .await?
        .ok_or_else(|| AppError::not_found(format!("media item {id} not found")))?;

    let path_str = match kind {
        "poster" => poster,
        "backdrop" => backdrop,
        other => {
            return Err(AppError::bad_request(format!(
                "unknown artwork type {other:?}; expected poster or backdrop"
            )))
        }
    };

    let path_str = path_str
        .ok_or_else(|| AppError::not_found(format!("no {kind} artwork available for item {id}")))?;

    let bytes = tokio::fs::read(&path_str)
        .await
        .map_err(|_| AppError::not_found(format!("artwork file for item {id} is unavailable")))?;
    let content_type = image_content_type(&path_str);

    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, content_type)
        .header(header::CONTENT_LENGTH, bytes.len())
        .body(Body::from(bytes))
        .map_err(|e| AppError::internal(e.to_string()))
}

async fn list_subtitles(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Json<Vec<SubtitleStream>>, AppError> {
    let item = db::get_item(&state.pool, id)
        .await?
        .ok_or_else(|| AppError::not_found(format!("media item {id} not found")))?;
    Ok(Json(item.subtitles))
}

fn vtt_response(bytes: Vec<u8>) -> Result<Response, AppError> {
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, HeaderValue::from_static("text/vtt"))
        .header(header::CONTENT_LENGTH, bytes.len())
        .body(Body::from(bytes))
        .map_err(|e| AppError::internal(e.to_string()))
}

async fn get_subtitle_track(
    State(state): State<AppState>,
    Path((id, track)): Path<(i64, u32)>,
) -> Result<Response, AppError> {
    let item = db::get_item(&state.pool, id)
        .await?
        .ok_or_else(|| AppError::not_found(format!("media item {id} not found")))?;

    // Sidecar subtitle file?
    let sidecars = db::get_sidecars(&state.pool, id).await?;
    if let Some(sidecar) = sidecars.iter().find(|s| s.index == track) {
        if sidecar.is_webvtt() {
            let bytes = tokio::fs::read(&sidecar.path).await?;
            return vtt_response(bytes);
        }
        let output = tokio::process::Command::new("ffmpeg")
            .args(["-v", "quiet", "-i"])
            .arg(&sidecar.path)
            .args(["-f", "webvtt", "-"])
            .output()
            .await
            .map_err(|e| AppError::internal(format!("failed to run ffmpeg: {e}")))?;
        if !output.status.success() {
            return Err(AppError::internal("ffmpeg failed to convert subtitle"));
        }
        return vtt_response(output.stdout);
    }

    // Embedded subtitle track?
    let embedded = db::get_embedded_subtitles(&state.pool, id).await?;
    if !embedded.iter().any(|s| s.index == track) {
        return Err(AppError::not_found(format!(
            "subtitle track {track} not found for item {id}"
        )));
    }

    let output = tokio::process::Command::new("ffmpeg")
        .args(["-v", "quiet", "-i"])
        .arg(&item.file_path)
        .args(["-map", &format!("0:{track}"), "-f", "webvtt", "-"])
        .output()
        .await
        .map_err(|e| AppError::internal(format!("failed to run ffmpeg: {e}")))?;
    if !output.status.success() {
        return Err(AppError::internal(
            "ffmpeg failed to extract embedded subtitle",
        ));
    }
    vtt_response(output.stdout)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_closed_range() {
        assert_eq!(
            parse_range("bytes=0-1023", 10_000),
            Ok(ByteRange {
                start: 0,
                end: 1023
            })
        );
    }

    #[test]
    fn parses_open_ended_range() {
        assert_eq!(
            parse_range("bytes=500-", 10_000),
            Ok(ByteRange {
                start: 500,
                end: 9_999
            })
        );
    }

    #[test]
    fn parses_suffix_range() {
        assert_eq!(
            parse_range("bytes=-500", 10_000),
            Ok(ByteRange {
                start: 9_500,
                end: 9_999
            })
        );
    }

    #[test]
    fn clamps_end_to_size() {
        assert_eq!(
            parse_range("bytes=0-99999", 10_000),
            Ok(ByteRange {
                start: 0,
                end: 9_999
            })
        );
    }

    #[test]
    fn rejects_unsatisfiable_and_malformed() {
        assert_eq!(
            parse_range("bytes=20000-", 10_000),
            Err(RangeError::Unsatisfiable)
        );
        assert_eq!(parse_range("bytes=0-", 0), Err(RangeError::Unsatisfiable));
        assert_eq!(
            parse_range("bytes=-0", 10_000),
            Err(RangeError::Unsatisfiable)
        );
        assert_eq!(
            parse_range("items=0-10", 10_000),
            Err(RangeError::Malformed)
        );
        assert_eq!(
            parse_range("bytes=abc-def", 10_000),
            Err(RangeError::Malformed)
        );
    }

    #[test]
    fn picks_first_of_multiple_ranges() {
        assert_eq!(
            parse_range("bytes=0-99,200-299", 10_000),
            Ok(ByteRange { start: 0, end: 99 })
        );
    }

    #[test]
    fn maps_content_types() {
        assert_eq!(content_type_for(Some("mov,mp4,m4a")), "video/mp4");
        assert_eq!(content_type_for(Some("matroska,webm")), "video/x-matroska");
        assert_eq!(content_type_for(Some("webm")), "video/webm");
        assert_eq!(content_type_for(Some("avi")), "video/x-msvideo");
        assert_eq!(content_type_for(Some("mpegts")), "video/mp2t");
        assert_eq!(content_type_for(None), "application/octet-stream");
    }
}
