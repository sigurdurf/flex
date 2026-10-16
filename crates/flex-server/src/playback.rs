//! Playback sessions: mode selection, the session manager, HLS session file
//! serving and the `?t=` scoped-token authorisation used by direct play.

use std::collections::HashMap;
use std::path::{Path as FsPath, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use axum::body::Body;
use axum::extract::{FromRequestParts, Path, Query, State};
use axum::http::request::Parts;
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::Response;
use axum::Json;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use flex_protocol::{
    playback_master_path, stream_path, ClientCapabilities, MediaItem, PlaybackMode,
    PlaybackSession, PlaybackStartRequest, StopPlaybackRequest, StreamKind, SystemInfo,
    API_VERSION,
};
use rand::RngCore;
use serde::Deserialize;
use tokio::process::Child;

use crate::api::AppState;
use crate::auth::AuthUser;
use crate::db;
use crate::error::AppError;
use crate::transcode;

/// Lifetime of a playback session before it is reclaimed.
pub const SESSION_TTL_SECS: i64 = 4 * 60 * 60;

/// Sessions with no file access for this long are garbage collected.
pub const IDLE_TIMEOUT: Duration = Duration::from_secs(120);

/// How often the background GC task runs.
pub const GC_INTERVAL: Duration = Duration::from_secs(30);

/// How long to wait for ffmpeg to produce the initial playlist.
const PLAYLIST_WAIT: Duration = Duration::from_secs(15);

/// Generate an ephemeral, session-scoped token (24 random bytes, base64url).
pub fn generate_session_token() -> String {
    let mut bytes = [0u8; 24];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

// --- Mode decision ----------------------------------------------------------

fn contains_ci(list: &[String], value: &str) -> bool {
    list.iter()
        .any(|candidate| candidate.eq_ignore_ascii_case(value))
}

/// The first comma-separated token of an ffprobe `format_name`, lowercased.
fn first_container(format_name: Option<&str>) -> Option<String> {
    format_name
        .and_then(|name| name.split(',').next())
        .map(|token| token.trim().to_ascii_lowercase())
        .filter(|token| !token.is_empty())
}

/// Canonical container token for an item, matching the names clients advertise
/// (`mp4`, `matroska`, `webm`, `mpegts`, ...).
///
/// An ffprobe `format_name` is a list of demuxer aliases and cannot tell
/// related formats apart: `.mp4` and `.mov` both report `mov,mp4,...`, and
/// `.mkv` and `.webm` both report `matroska,webm`. The file extension is
/// authoritative, so use it and fall back to the first `format_name` token
/// only when the extension is unrecognised.
fn container_of(item: &MediaItem) -> Option<String> {
    let ext = item
        .file_path
        .rsplit('.')
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();
    let token = match ext.as_str() {
        "mp4" | "m4v" => "mp4",
        "m4a" => "m4a",
        "mov" => "mov",
        "mkv" => "matroska",
        "webm" => "webm",
        "ts" | "m2ts" | "mts" => "mpegts",
        "avi" => "avi",
        "wmv" => "asf",
        "flv" => "flv",
        "mpg" | "mpeg" => "mpeg",
        _ => return first_container(item.container.as_deref()),
    };
    Some(token.to_string())
}

/// Containers that a typical client can consume directly while still claiming
/// HLS support; anything else (e.g. Matroska) forces a remux for HLS clients.
fn is_hls_friendly(container: &str) -> bool {
    matches!(container, "mov" | "mp4" | "m4a" | "m4v" | "mpegts" | "ts")
}

/// Whether every audio stream's codec is decodable by the client.
pub fn audio_supported(item: &MediaItem, caps: &ClientCapabilities) -> bool {
    item.audio
        .iter()
        .all(|stream| contains_ci(&caps.audio_codecs, &stream.codec))
}

/// Choose the cheapest playback mode the client can handle.
///
/// Empty capability lists are treated as "supports nothing", forcing remux or
/// transcode so callers can drive each branch deterministically.
pub fn decide_mode(item: &MediaItem, caps: &ClientCapabilities) -> PlaybackMode {
    let container = container_of(item);
    let container_ok = container
        .as_deref()
        .map(|c| contains_ci(&caps.containers, c))
        .unwrap_or(false);
    let video_ok = item
        .video
        .as_ref()
        .map(|video| contains_ci(&caps.video_codecs, &video.codec))
        .unwrap_or(true);
    let audio_ok = audio_supported(item, caps);
    let resolution_ok = item
        .video
        .as_ref()
        .map(|video| {
            caps.max_width.is_none_or(|w| video.width <= w)
                && caps.max_height.is_none_or(|h| video.height <= h)
        })
        .unwrap_or(true);
    let bitrate_ok = match (
        item.video.as_ref().and_then(|v| v.bitrate),
        caps.max_bitrate,
    ) {
        (Some(bitrate), Some(max)) => bitrate <= max,
        _ => true,
    };
    let hls_friendly = container.as_deref().map(is_hls_friendly).unwrap_or(false);

    if container_ok && video_ok && audio_ok && resolution_ok && bitrate_ok {
        if caps.hls && !hls_friendly {
            return PlaybackMode::Remux;
        }
        return PlaybackMode::Direct;
    }

    if !video_ok || !resolution_ok || !bitrate_ok {
        return PlaybackMode::Transcode;
    }

    PlaybackMode::Remux
}

// --- HLS path sanitisation --------------------------------------------------

/// Validate a client-supplied HLS file name.
///
/// The file component is never used to build an arbitrary path: it must be a
/// bare name with a known playlist/segment extension and no separators.
pub fn sanitize_hls_file(file: &str) -> Result<&str, AppError> {
    let invalid = file.is_empty()
        || file.contains('/')
        || file.contains('\\')
        || file.contains("..")
        || file.contains('\0')
        || file.starts_with('.');
    let known = file.ends_with(".m3u8")
        || file.ends_with(".ts")
        || file.ends_with(".m4s")
        || file == "init.mp4";
    let safe_chars = file
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'));
    if invalid || !known || !safe_chars {
        return Err(AppError::not_found("unknown playback file"));
    }
    Ok(file)
}

/// Content type for an HLS session artifact.
pub fn hls_content_type(file: &str) -> &'static str {
    if file.ends_with(".m3u8") {
        "application/vnd.apple.mpegurl"
    } else if file.ends_with(".ts") {
        "video/mp2t"
    } else if file.ends_with(".m4s") {
        "video/iso.segment"
    } else if file.ends_with(".mp4") {
        "video/mp4"
    } else {
        "application/octet-stream"
    }
}

// --- Session manager --------------------------------------------------------

struct Session {
    id: String,
    token: String,
    item_id: i64,
    mode: PlaybackMode,
    stream_kind: StreamKind,
    dir: PathBuf,
    child: Option<Child>,
    expires_at: i64,
    last_access: Instant,
}

/// In-memory registry of live playback sessions.
pub struct SessionManager {
    sessions: Mutex<HashMap<String, Session>>,
    max_transcode_sessions: usize,
    cache_dir: PathBuf,
}

impl SessionManager {
    /// Create a manager rooted at `cache_dir` with the given concurrency cap.
    pub fn new(cache_dir: PathBuf, max_transcode_sessions: usize) -> Self {
        Self {
            sessions: Mutex::new(HashMap::new()),
            max_transcode_sessions: max_transcode_sessions.max(1),
            cache_dir,
        }
    }

    /// Root directory holding all playback session directories.
    fn playback_root(&self) -> PathBuf {
        self.cache_dir.join("playback")
    }

    /// The per-session working directory.
    fn session_dir(&self, session_id: &str) -> PathBuf {
        self.playback_root().join(session_id)
    }

    /// Remove any stale session directories left by a previous run.
    pub async fn sweep(&self) -> std::io::Result<()> {
        let root = self.playback_root();
        if tokio::fs::metadata(&root).await.is_ok() {
            tokio::fs::remove_dir_all(&root).await?;
        }
        tokio::fs::create_dir_all(&root).await
    }

    /// Insert a session, rejecting when the HLS concurrency cap is reached.
    ///
    /// HLS sessions (remux/transcode) are bounded; direct sessions are cheap.
    fn insert_checked(&self, session: Session) -> Result<(), AppError> {
        let mut sessions = self.sessions.lock().expect("session mutex poisoned");
        if session.stream_kind == StreamKind::Hls {
            let active = sessions
                .values()
                .filter(|s| s.stream_kind == StreamKind::Hls)
                .count();
            if active >= self.max_transcode_sessions {
                return Err(AppError::service_unavailable(
                    "transcode session limit reached; try again later",
                ));
            }
        }
        sessions.insert(session.id.clone(), session);
        Ok(())
    }

    fn set_child(&self, session_id: &str, child: Child) {
        let mut sessions = self.sessions.lock().expect("session mutex poisoned");
        if let Some(session) = sessions.get_mut(session_id) {
            session.child = Some(child);
        } else {
            // The session was stopped while we were spawning; reap the child.
            drop(sessions);
            let mut child = child;
            tokio::spawn(async move { transcode::kill(&mut child).await });
        }
    }

    /// Authorise access to a session's HLS artifacts, returning its directory.
    fn authorize(&self, session_id: &str, token: &str) -> Result<PathBuf, AppError> {
        let mut sessions = self.sessions.lock().expect("session mutex poisoned");
        let now = Instant::now();
        match sessions.get_mut(session_id) {
            Some(session) if db::now_secs() >= session.expires_at => {
                Err(AppError::not_found("playback session expired"))
            }
            Some(session) if session.token != token => {
                Err(AppError::unauthorized("invalid session token"))
            }
            Some(session) => {
                session.last_access = now;
                Ok(session.dir.clone())
            }
            None => Err(AppError::not_found("playback session not found")),
        }
    }

    /// Resolve a direct-play session token to the item it authorises.
    fn authorize_stream_token(&self, token: &str) -> Option<i64> {
        let mut sessions = self.sessions.lock().expect("session mutex poisoned");
        let now = Instant::now();
        for session in sessions.values_mut() {
            if session.stream_kind == StreamKind::File
                && session.token == token
                && db::now_secs() < session.expires_at
            {
                session.last_access = now;
                return Some(session.item_id);
            }
        }
        None
    }

    /// Stop a session: kill ffmpeg and delete its directory. Returns whether a
    /// session existed.
    async fn stop(&self, session_id: &str) -> bool {
        let session = {
            let mut sessions = self.sessions.lock().expect("session mutex poisoned");
            sessions.remove(session_id)
        };
        let Some(mut session) = session else {
            return false;
        };
        if let Some(child) = session.child.as_mut() {
            transcode::kill(child).await;
        }
        let _ = tokio::fs::remove_dir_all(&session.dir).await;
        true
    }

    /// Reclaim sessions idle for longer than `idle`, returning how many.
    pub async fn gc(&self, idle: Duration) -> usize {
        let now = Instant::now();
        let now_secs = db::now_secs();
        let expired: Vec<Session> = {
            let mut sessions = self.sessions.lock().expect("session mutex poisoned");
            let ids: Vec<String> = sessions
                .iter()
                .filter(|(_, s)| {
                    now.duration_since(s.last_access) > idle || now_secs >= s.expires_at
                })
                .map(|(id, _)| id.clone())
                .collect();
            ids.into_iter()
                .filter_map(|id| sessions.remove(&id))
                .collect()
        };
        let count = expired.len();
        for mut session in expired {
            tracing::debug!(session = %session.id, mode = ?session.mode, "reclaiming playback session");
            if let Some(child) = session.child.as_mut() {
                transcode::kill(child).await;
            }
            let _ = tokio::fs::remove_dir_all(&session.dir).await;
        }
        count
    }
}

// --- Handlers ---------------------------------------------------------------

/// Resolve the effective start position for a playback request.
///
/// An explicit `requested` value always wins. Otherwise the user's saved watch
/// state is used, but only when it is resumable (not watched and past zero).
pub async fn resolve_start_position(
    pool: &sqlx::SqlitePool,
    user_id: i64,
    item_id: i64,
    requested: Option<u64>,
) -> Result<Option<u64>, sqlx::Error> {
    match requested {
        Some(ms) => Ok(Some(ms)),
        None => Ok(db::get_watch_state(pool, user_id, item_id)
            .await?
            .filter(|state| !state.watched && state.position_ms > 0)
            .map(|state| state.position_ms)),
    }
}

/// `POST /api/v1/playback/start`
pub async fn start(
    auth: AuthUser,
    State(state): State<AppState>,
    Json(request): Json<PlaybackStartRequest>,
) -> Result<Json<PlaybackSession>, AppError> {
    let item = db::get_item(&state.pool, request.item_id)
        .await?
        .ok_or_else(|| AppError::not_found(format!("media item {} not found", request.item_id)))?;

    let start_ms = resolve_start_position(
        &state.pool,
        auth.user.id,
        item.id,
        request.start_position_ms,
    )
    .await?;

    let mode = decide_mode(&item, &request.capabilities);
    let session_id = uuid::Uuid::new_v4().to_string();
    let token = generate_session_token();
    let expires_at = db::now_secs() + SESSION_TTL_SECS;
    let audio_stream_index = request
        .audio_track
        .or_else(|| item.audio.first().map(|a| a.index));
    let subtitle_stream_index = request.subtitle_track;

    if mode == PlaybackMode::Direct {
        // Direct play serves the original file as-is. The wire protocol has no
        // field to carry a resume offset, so a saved position is not applied
        // here: the client is expected to seek after opening the file (it can
        // read the position from `GET /progress/{item_id}`).
        let session = Session {
            id: session_id.clone(),
            token: token.clone(),
            item_id: item.id,
            mode,
            stream_kind: StreamKind::File,
            dir: state.sessions.session_dir(&session_id),
            child: None,
            expires_at,
            last_access: Instant::now(),
        };
        state.sessions.insert_checked(session)?;

        let url = format!("{}?t={}", stream_path(item.id), token);
        return Ok(Json(PlaybackSession {
            session_id,
            mode,
            stream_kind: StreamKind::File,
            url,
            mime: Some(
                crate::api::content_type_for_path(&item.file_path, item.container.as_deref())
                    .to_string(),
            ),
            duration_ms: item.duration_ms,
            audio_stream_index,
            subtitle_stream_index,
            session_token: token,
            expires_at: Some(expires_at),
        }));
    }

    // Remux / transcode require ffmpeg.
    if !state.transcode.available {
        return Err(AppError::service_unavailable(
            "ffmpeg is unavailable; remux and transcode are disabled",
        ));
    }
    if mode == PlaybackMode::Transcode && !state.config.transcode_enabled {
        return Err(AppError::service_unavailable(
            "transcoding is disabled on this server",
        ));
    }

    let caps = &request.capabilities;
    let args = match mode {
        PlaybackMode::Remux => {
            let video_codec = item
                .video
                .as_ref()
                .map(|v| v.codec.as_str())
                .unwrap_or("h264");
            let copy_audio = audio_supported(&item, caps);
            transcode::remux_args(&item.file_path, video_codec, copy_audio, start_ms)
        }
        _ => {
            let want_hevc = item
                .video
                .as_ref()
                .map(|v| v.codec.eq_ignore_ascii_case("hevc"))
                .unwrap_or(false)
                && contains_ci(&caps.video_codecs, "hevc");
            let scale = transcode::scale_filter(caps.max_width, caps.max_height);
            transcode::transcode_args(
                &state.transcode,
                &item.file_path,
                want_hevc,
                scale.as_deref(),
                caps.max_bitrate,
                start_ms,
            )
        }
    };

    let dir = state.sessions.session_dir(&session_id);
    let session = Session {
        id: session_id.clone(),
        token: token.clone(),
        item_id: item.id,
        mode,
        stream_kind: StreamKind::Hls,
        dir: dir.clone(),
        child: None,
        expires_at,
        last_access: Instant::now(),
    };
    state.sessions.insert_checked(session)?;

    if let Err(err) = tokio::fs::create_dir_all(&dir).await {
        state.sessions.stop(&session_id).await;
        return Err(AppError::internal(format!(
            "failed to create session directory: {err}"
        )));
    }

    let mut child = match transcode::spawn(&dir, &args) {
        Ok(child) => child,
        Err(err) => {
            state.sessions.stop(&session_id).await;
            return Err(AppError::service_unavailable(format!(
                "failed to start ffmpeg: {err}"
            )));
        }
    };
    tracing::info!(
        session = %session_id,
        mode = ?mode,
        "spawned ffmpeg: ffmpeg {}",
        args.join(" ")
    );
    transcode::log_stderr(&mut child, session_id.clone());
    state.sessions.set_child(&session_id, child);

    if !wait_for_playlist(&dir, PLAYLIST_WAIT).await {
        state.sessions.stop(&session_id).await;
        return Err(AppError::service_unavailable(
            "ffmpeg did not produce a playlist in time",
        ));
    }

    let url = format!("{}?t={}", playback_master_path(&session_id), token);
    Ok(Json(PlaybackSession {
        session_id,
        mode,
        stream_kind: StreamKind::Hls,
        url,
        mime: Some("application/vnd.apple.mpegurl".to_string()),
        duration_ms: item.duration_ms,
        audio_stream_index,
        subtitle_stream_index,
        session_token: token,
        expires_at: Some(expires_at),
    }))
}

/// `POST /api/v1/playback/stop`
pub async fn stop(
    _auth: AuthUser,
    State(state): State<AppState>,
    Json(request): Json<StopPlaybackRequest>,
) -> Result<StatusCode, AppError> {
    state.sessions.stop(&request.session_id).await;
    Ok(StatusCode::NO_CONTENT)
}

async fn wait_for_playlist(dir: &FsPath, timeout: Duration) -> bool {
    let playlist = dir.join("master.m3u8");
    let deadline = Instant::now() + timeout;
    loop {
        if tokio::fs::metadata(&playlist)
            .await
            .map(|m| m.is_file() && m.len() > 0)
            .unwrap_or(false)
        {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(150)).await;
    }
}

/// Carry the scoped token onto each request a native HLS player makes. Query
/// parameters on a playlist URL are not inherited by relative segment URLs.
fn scoped_playlist(playlist: &str, token: &str) -> Result<String, AppError> {
    let sign = |file: &str| -> Result<String, AppError> {
        // Only server-generated session filenames are allowed. Never put a
        // session token on an external URL supplied by playlist contents.
        let file = sanitize_hls_file(file)?;
        Ok(format!("{file}?t={token}"))
    };
    let mut result = String::with_capacity(playlist.len());
    for line in playlist.lines() {
        if let Some((prefix, value)) = line.split_once("URI=\"") {
            let (file, suffix) = value
                .split_once('"')
                .ok_or_else(|| AppError::internal("invalid HLS URI attribute"))?;
            result.push_str(&format!("{prefix}URI=\"{}\"{suffix}", sign(file)?));
        } else if !line.is_empty() && !line.starts_with('#') {
            result.push_str(&sign(line)?);
        } else {
            result.push_str(line);
        }
        result.push('\n');
    }
    Ok(result)
}

/// Query parameters for HLS requests: the scoped session token.
#[derive(Debug, Default, Deserialize)]
pub struct TokenQuery {
    #[serde(default)]
    pub t: Option<String>,
}

/// `GET /api/v1/playback/{session_id}/master.m3u8`
pub async fn master(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    Query(query): Query<TokenQuery>,
    headers: HeaderMap,
) -> Result<Response, AppError> {
    serve_hls(state, session_id, "master.m3u8".to_string(), query, headers).await
}

/// `GET /api/v1/playback/{session_id}/{file}`
pub async fn segment(
    State(state): State<AppState>,
    Path((session_id, file)): Path<(String, String)>,
    Query(query): Query<TokenQuery>,
    headers: HeaderMap,
) -> Result<Response, AppError> {
    serve_hls(state, session_id, file, query, headers).await
}

async fn serve_hls(
    state: AppState,
    session_id: String,
    file: String,
    query: TokenQuery,
    headers: HeaderMap,
) -> Result<Response, AppError> {
    let file = sanitize_hls_file(&file)?.to_string();
    let token = query
        .t
        .ok_or_else(|| AppError::unauthorized("missing session token"))?;
    let dir = state.sessions.authorize(&session_id, &token)?;
    let path = dir.join(&file);

    let metadata = match tokio::fs::metadata(&path).await {
        Ok(metadata) if metadata.is_file() => metadata,
        _ => return Err(AppError::not_found("playback file not found")),
    };
    let playlist = if file.ends_with(".m3u8") {
        let contents = tokio::fs::read_to_string(&path)
            .await
            .map_err(|error| AppError::internal(format!("failed to read HLS playlist: {error}")))?;
        Some(scoped_playlist(&contents, &token)?.into_bytes())
    } else {
        None
    };
    // Ranges and Content-Length must describe the signed bytes, not the file.
    let size = playlist
        .as_ref()
        .map_or(metadata.len(), |bytes| bytes.len() as u64);
    let cache_control = if playlist.is_some() {
        "no-store"
    } else {
        "private"
    };
    let content_type = hls_content_type(&file);
    let range_header = headers
        .get(header::RANGE)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());

    match range_header {
        Some(raw) => match crate::api::parse_range(&raw, size) {
            Ok(range) => {
                let len = range.end - range.start + 1;
                let body = match playlist {
                    Some(bytes) => {
                        Body::from(bytes[range.start as usize..=range.end as usize].to_vec())
                    }
                    None => crate::api::file_body(&path, range.start, Some(len)).await?,
                };
                Response::builder()
                    .status(StatusCode::PARTIAL_CONTENT)
                    .header(header::CONTENT_TYPE, content_type)
                    .header(header::CACHE_CONTROL, cache_control)
                    .header(header::ACCEPT_RANGES, "bytes")
                    .header(
                        header::CONTENT_RANGE,
                        format!("bytes {}-{}/{}", range.start, range.end, size),
                    )
                    .header(header::CONTENT_LENGTH, len)
                    .body(body)
                    .map_err(|e| AppError::internal(e.to_string()))
            }
            Err(crate::api::RangeError::Unsatisfiable) => Response::builder()
                .status(StatusCode::RANGE_NOT_SATISFIABLE)
                .header(header::CONTENT_RANGE, format!("bytes */{size}"))
                .header(header::ACCEPT_RANGES, "bytes")
                .body(Body::empty())
                .map_err(|e| AppError::internal(e.to_string())),
            Err(crate::api::RangeError::Malformed) => {
                Err(AppError::bad_request("malformed Range header"))
            }
        },
        None => {
            let body = match playlist {
                Some(bytes) => Body::from(bytes),
                None => crate::api::file_body(&path, 0, None).await?,
            };
            Response::builder()
                .status(StatusCode::OK)
                .header(header::CONTENT_TYPE, content_type)
                .header(header::CACHE_CONTROL, cache_control)
                .header(header::ACCEPT_RANGES, "bytes")
                .header(header::CONTENT_LENGTH, size)
                .body(body)
                .map_err(|e| AppError::internal(e.to_string()))
        }
    }
}

/// `GET /api/v1/system/info`
pub async fn system_info(State(state): State<AppState>) -> Json<SystemInfo> {
    let caps = &state.transcode;
    Json(SystemInfo {
        version: env!("CARGO_PKG_VERSION").to_string(),
        api_version: API_VERSION.to_string(),
        hwaccels: caps.hwaccels.clone(),
        hw_encoders: caps.hw_encoders.clone(),
        transcode_enabled: caps.available && state.config.transcode_enabled,
    })
}

// --- Direct-play scoped access ---------------------------------------------

/// Request extractor for `/items/{id}/stream` accepting **either** a bearer
/// token or a valid direct-play `?t=` session token.
pub struct StreamAccess {
    /// `Some(item_id)` when authorised by a scoped session token (bound to that
    /// item); `None` when authorised by a bearer token.
    pub via_session_item: Option<i64>,
}

impl FromRequestParts<AppState> for StreamAccess {
    type Rejection = AppError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        if AuthUser::from_request_parts(parts, state).await.is_ok() {
            return Ok(StreamAccess {
                via_session_item: None,
            });
        }

        let query = Query::<TokenQuery>::from_request_parts(parts, state)
            .await
            .map_err(|_| AppError::unauthorized("invalid query string"))?;
        if let Some(token) = query.t.as_deref() {
            if let Some(item_id) = state.sessions.authorize_stream_token(token) {
                return Ok(StreamAccess {
                    via_session_item: Some(item_id),
                });
            }
        }
        Err(AppError::unauthorized("missing or invalid credentials"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use flex_protocol::{AudioStream, MediaItem, VideoStream};

    fn caps(
        containers: &[&str],
        video: &[&str],
        audio: &[&str],
        max_width: Option<u32>,
        max_height: Option<u32>,
        max_bitrate: Option<u64>,
        hls: bool,
    ) -> ClientCapabilities {
        ClientCapabilities {
            containers: containers.iter().map(|s| s.to_string()).collect(),
            video_codecs: video.iter().map(|s| s.to_string()).collect(),
            audio_codecs: audio.iter().map(|s| s.to_string()).collect(),
            max_width,
            max_height,
            max_bitrate,
            hls,
            hdr: false,
        }
    }

    fn item(ext: &str, container: &str, video_codec: &str, audio_codec: &str) -> MediaItem {
        MediaItem {
            id: 1,
            library_id: 1,
            kind: flex_protocol::MediaKind::Movie,
            title: "Test".into(),
            sort_title: "test".into(),
            year: None,
            overview: None,
            file_path: format!("/media/test.{ext}"),
            file_size: 1,
            container: Some(container.into()),
            duration_ms: Some(1000),
            video: Some(VideoStream {
                codec: video_codec.into(),
                width: 1920,
                height: 1080,
                fps: Some(24.0),
                bitrate: Some(5_000_000),
            }),
            audio: vec![AudioStream {
                index: 1,
                codec: audio_codec.into(),
                channels: Some(2),
                language: None,
                title: None,
                bitrate: None,
            }],
            subtitles: vec![],
            added_at: 0,
            updated_at: 0,
            poster_url: None,
            backdrop_url: None,
            series_id: None,
            season: None,
            episode: None,
        }
    }

    #[test]
    fn decides_direct() {
        // Real clients advertise "mp4", not ffprobe's first alias "mov".
        let item = item("mp4", "mov,mp4,m4a,3gp,3g2,mj2", "h264", "aac");
        let caps = caps(&["mp4"], &["h264"], &["aac"], None, None, None, true);
        assert_eq!(decide_mode(&item, &caps), PlaybackMode::Direct);
    }

    #[test]
    fn extension_disambiguates_mp4_from_mov() {
        // ffprobe reports the same format_name for .mp4 and .mov; the extension
        // decides, so a client that only claims "mp4" remuxes a .mov.
        let caps = caps(&["mp4"], &["h264"], &["aac"], None, None, None, false);
        let same_format = "mov,mp4,m4a,3gp,3g2,mj2";
        assert_eq!(
            decide_mode(&item("mp4", same_format, "h264", "aac"), &caps),
            PlaybackMode::Direct
        );
        assert_eq!(
            decide_mode(&item("mov", same_format, "h264", "aac"), &caps),
            PlaybackMode::Remux
        );
    }

    #[test]
    fn extension_disambiguates_webm_from_mkv() {
        let caps = caps(&["webm"], &["vp9"], &["opus"], None, None, None, false);
        let same_format = "matroska,webm";
        assert_eq!(
            decide_mode(&item("webm", same_format, "vp9", "opus"), &caps),
            PlaybackMode::Direct
        );
        assert_eq!(
            decide_mode(&item("mkv", same_format, "vp9", "opus"), &caps),
            PlaybackMode::Remux
        );
    }

    #[test]
    fn decides_remux_for_unsupported_container() {
        let item = item("mkv", "matroska,webm", "h264", "aac");
        let caps = caps(&[], &["h264"], &["aac"], None, None, None, true);
        assert_eq!(decide_mode(&item, &caps), PlaybackMode::Remux);
    }

    #[test]
    fn decides_remux_for_unsupported_audio() {
        let item = item("mp4", "mov,mp4", "h264", "eac3");
        let caps = caps(&["mp4"], &["h264"], &[], None, None, None, true);
        assert_eq!(decide_mode(&item, &caps), PlaybackMode::Remux);
    }

    #[test]
    fn decides_remux_when_hls_client_needs_non_hls_container() {
        // Client claims Matroska support but only plays HLS.
        let item = item("mkv", "matroska,webm", "h264", "aac");
        let caps = caps(&["matroska"], &["h264"], &["aac"], None, None, None, true);
        assert_eq!(decide_mode(&item, &caps), PlaybackMode::Remux);
    }

    #[test]
    fn decides_transcode_for_resolution() {
        let item = item("mp4", "mov,mp4", "h264", "aac");
        let caps = caps(&["mp4"], &["h264"], &["aac"], None, Some(480), None, true);
        assert_eq!(decide_mode(&item, &caps), PlaybackMode::Transcode);
    }

    #[test]
    fn decides_transcode_for_bitrate() {
        let item = item("mp4", "mov,mp4", "h264", "aac");
        let caps = caps(
            &["mp4"],
            &["h264"],
            &["aac"],
            None,
            None,
            Some(1_000_000),
            true,
        );
        assert_eq!(decide_mode(&item, &caps), PlaybackMode::Transcode);
    }

    #[test]
    fn decides_transcode_for_unsupported_video_codec() {
        let item = item("mp4", "mov,mp4", "hevc", "aac");
        let caps = caps(&["mp4"], &["av1"], &["aac"], None, None, None, true);
        assert_eq!(decide_mode(&item, &caps), PlaybackMode::Transcode);
    }

    #[test]
    fn empty_capabilities_force_reencoding() {
        let item = item("mp4", "mov,mp4", "h264", "aac");
        let caps = caps(&[], &[], &[], None, None, None, false);
        assert_eq!(decide_mode(&item, &caps), PlaybackMode::Transcode);
    }

    #[test]
    fn sanitizes_hls_files() {
        assert_eq!(sanitize_hls_file("master.m3u8").unwrap(), "master.m3u8");
        assert_eq!(sanitize_hls_file("seg_00001.ts").unwrap(), "seg_00001.ts");
        assert_eq!(sanitize_hls_file("init.mp4").unwrap(), "init.mp4");
        assert!(sanitize_hls_file("seg_00002.m4s").is_ok());

        assert!(sanitize_hls_file("..").is_err());
        assert!(sanitize_hls_file("../../etc/passwd").is_err());
        assert!(sanitize_hls_file("a/b").is_err());
        assert!(sanitize_hls_file("a\\b.ts").is_err());
        assert!(sanitize_hls_file("evil.sh").is_err());
        assert!(sanitize_hls_file("").is_err());
    }

    #[test]
    fn signs_hls_segments_without_changing_playlist_tags() {
        let playlist = "#EXTM3U\n#EXT-X-VERSION:3\n#EXTINF:6.0,\nseg_00001.ts\n";
        assert_eq!(
            scoped_playlist(playlist, "session-token").unwrap(),
            "#EXTM3U\n#EXT-X-VERSION:3\n#EXTINF:6.0,\nseg_00001.ts?t=session-token\n"
        );
    }

    #[test]
    fn signs_fmp4_init_and_media_segments() {
        let playlist = "#EXTM3U\n#EXT-X-MAP:URI=\"init.mp4\",BYTERANGE=\"100@0\"\nseg_00001.m4s\n";
        assert_eq!(
            scoped_playlist(playlist, "session-token").unwrap(),
            "#EXTM3U\n#EXT-X-MAP:URI=\"init.mp4?t=session-token\",BYTERANGE=\"100@0\"\nseg_00001.m4s?t=session-token\n"
        );
    }

    #[test]
    fn refuses_to_sign_external_or_unsafe_playlist_urls() {
        for playlist in [
            "#EXTM3U\nhttps://other.example/video.ts\n",
            "#EXTM3U\n../video.ts\n",
            "#EXTM3U\n#EXT-X-MAP:URI=\"https://other.example/init.mp4\"\n",
            "#EXTM3U\n#EXT-X-MAP:URI=\"init.mp4\n",
        ] {
            assert!(scoped_playlist(playlist, "private-token").is_err());
        }
    }

    #[tokio::test]
    async fn serves_signed_playlist_ranges_and_authorized_segments() {
        use std::sync::Arc;

        let temp = tempfile::tempdir().unwrap();
        let token = "session-token";
        let playlist = "#EXTM3U\n#EXTINF:6.0,\nseg_00001.ts\n";
        let signed = scoped_playlist(playlist, token).unwrap();
        tokio::fs::write(temp.path().join("master.m3u8"), playlist)
            .await
            .unwrap();
        tokio::fs::write(temp.path().join("seg_00001.ts"), b"test-segment")
            .await
            .unwrap();
        let sessions = Arc::new(manager());
        let mut session = direct_session("web-test", token, 1);
        session.dir = temp.path().into();
        session.stream_kind = StreamKind::Hls;
        sessions.insert_checked(session).unwrap();
        let state = AppState {
            pool: db::test_pool().await,
            config: Arc::new(crate::config::Config::default()),
            rate_limiter: Arc::new(crate::auth::LoginRateLimiter::new()),
            sessions,
            transcode: Arc::new(crate::transcode::TranscodeCapabilities::default()),
            subtitles: Arc::new(crate::subtitles::SubtitleRegistry::new()),
        };
        let serve = |file: &str, headers, token: Option<&str>| {
            serve_hls(
                state.clone(),
                "web-test".into(),
                file.into(),
                TokenQuery {
                    t: token.map(str::to_string),
                },
                headers,
            )
        };

        let response = serve("master.m3u8", HeaderMap::new(), Some(token))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers()[header::CONTENT_LENGTH],
            signed.len().to_string()
        );
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        let bytes = axum::body::to_bytes(response.into_body(), 4096)
            .await
            .unwrap();
        assert_eq!(bytes.as_ref(), signed.as_bytes());

        let mut headers = HeaderMap::new();
        headers.insert(header::RANGE, "bytes=0-15".parse().unwrap());
        let response = serve("master.m3u8", headers, Some(token)).await.unwrap();
        assert_eq!(response.status(), StatusCode::PARTIAL_CONTENT);
        assert_eq!(
            response.headers()[header::CONTENT_RANGE],
            format!("bytes 0-15/{}", signed.len())
        );
        assert_eq!(response.headers()[header::CONTENT_LENGTH], "16");
        let bytes = axum::body::to_bytes(response.into_body(), 4096)
            .await
            .unwrap();
        assert_eq!(bytes.as_ref(), &signed.as_bytes()[..16]);

        let response = serve("seg_00001.ts", HeaderMap::new(), Some(token))
            .await
            .unwrap();
        let bytes = axum::body::to_bytes(response.into_body(), 4096)
            .await
            .unwrap();
        assert_eq!(bytes.as_ref(), b"test-segment");
        assert!(matches!(
            serve("seg_00001.ts", HeaderMap::new(), None).await,
            Err(AppError::Unauthorized(_))
        ));
        assert!(matches!(
            serve("seg_00001.ts", HeaderMap::new(), Some("wrong-token")).await,
            Err(AppError::Unauthorized(_))
        ));
    }

    #[test]
    fn maps_hls_content_types() {
        assert_eq!(
            hls_content_type("master.m3u8"),
            "application/vnd.apple.mpegurl"
        );
        assert_eq!(hls_content_type("seg_00001.ts"), "video/mp2t");
        assert_eq!(hls_content_type("seg_00001.m4s"), "video/iso.segment");
        assert_eq!(hls_content_type("init.mp4"), "video/mp4");
    }

    #[test]
    fn session_tokens_are_unique_and_urlsafe() {
        let a = generate_session_token();
        let b = generate_session_token();
        assert_ne!(a, b);
        let decoded = URL_SAFE_NO_PAD.decode(a.as_bytes()).unwrap();
        assert_eq!(decoded.len(), 24);
    }

    fn manager() -> SessionManager {
        SessionManager::new(PathBuf::from("/tmp/flex-test-cache"), 2)
    }

    fn direct_session(id: &str, token: &str, item_id: i64) -> Session {
        Session {
            id: id.into(),
            token: token.into(),
            item_id,
            mode: PlaybackMode::Direct,
            stream_kind: StreamKind::File,
            dir: PathBuf::from("/tmp/flex-test-cache/playback").join(id),
            child: None,
            expires_at: db::now_secs() + SESSION_TTL_SECS,
            last_access: Instant::now(),
        }
    }

    #[test]
    fn authorizes_matching_session_token() {
        let manager = manager();
        manager
            .insert_checked(direct_session("s1", "tok1", 7))
            .unwrap();
        assert_eq!(
            manager.authorize("s1", "tok1").unwrap(),
            PathBuf::from("/tmp/flex-test-cache/playback/s1")
        );
        assert!(matches!(
            manager.authorize("s1", "wrong"),
            Err(AppError::Unauthorized(_))
        ));
        assert!(matches!(
            manager.authorize("missing", "tok1"),
            Err(AppError::NotFound(_))
        ));
    }

    #[test]
    fn stream_token_binds_to_item() {
        let manager = manager();
        manager
            .insert_checked(direct_session("s1", "tok1", 42))
            .unwrap();
        assert_eq!(manager.authorize_stream_token("tok1"), Some(42));
        assert_eq!(manager.authorize_stream_token("nope"), None);
    }

    #[test]
    fn enforces_transcode_concurrency_limit() {
        let manager = manager();
        for i in 0..2 {
            let mut session = direct_session(&format!("s{i}"), &format!("tok{i}"), 1);
            session.stream_kind = StreamKind::Hls;
            manager.insert_checked(session).unwrap();
        }
        let mut third = direct_session("s2", "tok2", 1);
        third.stream_kind = StreamKind::Hls;
        assert!(matches!(
            manager.insert_checked(third),
            Err(AppError::ServiceUnavailable(_))
        ));
        // Direct sessions are not bounded by the transcode cap.
        manager
            .insert_checked(direct_session("direct", "tokd", 1))
            .unwrap();
    }

    #[tokio::test]
    async fn gc_reclaims_idle_and_expired_sessions() {
        let manager = manager();

        let mut stale = direct_session("stale", "t-stale", 1);
        stale.last_access = Instant::now() - Duration::from_secs(300);
        manager.insert_checked(stale).unwrap();

        let mut expired = direct_session("expired", "t-expired", 1);
        expired.expires_at = db::now_secs() - 1;
        manager.insert_checked(expired).unwrap();

        manager
            .insert_checked(direct_session("fresh", "t-fresh", 1))
            .unwrap();

        let removed = manager.gc(Duration::from_secs(60)).await;
        assert_eq!(removed, 2);
        assert!(manager.authorize("fresh", "t-fresh").is_ok());
        assert!(manager.authorize("stale", "t-stale").is_err());
        assert!(manager.authorize("expired", "t-expired").is_err());
    }

    async fn seed_user(pool: &sqlx::SqlitePool) -> i64 {
        db::create_user(
            pool,
            "alice",
            "not-a-real-hash",
            flex_protocol::UserRole::User,
        )
        .await
        .unwrap()
        .id
    }

    async fn seed_item(pool: &sqlx::SqlitePool) -> i64 {
        let library = db::upsert_library(
            pool,
            "Movies",
            "/media/movies",
            flex_protocol::LibraryKind::Movies,
        )
        .await
        .unwrap();
        let item = crate::model::NewMediaItem {
            library_id: library,
            kind: flex_protocol::MediaKind::Movie,
            title: "Test".into(),
            sort_title: "test".into(),
            year: None,
            overview: None,
            file_path: "/media/movies/test.mkv".into(),
            file_size: 1,
            container: Some("matroska".into()),
            duration_ms: Some(100_000),
            streams: Default::default(),
            sidecars: Vec::new(),
            poster_path: None,
            backdrop_path: None,
            series_id: None,
            season: None,
            episode: None,
        };
        db::upsert_media(pool, &item).await.unwrap().0
    }

    #[tokio::test]
    async fn resumes_from_saved_position_when_request_omits_it() {
        let pool = db::test_pool().await;
        let user = seed_user(&pool).await;
        let item_id = seed_item(&pool).await;
        db::upsert_watch_state(&pool, user, item_id, 42_000, Some(100_000), false, 10)
            .await
            .unwrap();

        let resolved = resolve_start_position(&pool, user, item_id, None)
            .await
            .unwrap();
        assert_eq!(resolved, Some(42_000));
    }

    #[tokio::test]
    async fn explicit_request_position_wins() {
        let pool = db::test_pool().await;
        let user = seed_user(&pool).await;
        let item_id = seed_item(&pool).await;
        db::upsert_watch_state(&pool, user, item_id, 42_000, Some(100_000), false, 10)
            .await
            .unwrap();

        let resolved = resolve_start_position(&pool, user, item_id, Some(7_000))
            .await
            .unwrap();
        assert_eq!(resolved, Some(7_000));
    }

    #[tokio::test]
    async fn watched_item_does_not_resume() {
        let pool = db::test_pool().await;
        let user = seed_user(&pool).await;
        let item_id = seed_item(&pool).await;
        db::upsert_watch_state(&pool, user, item_id, 99_000, Some(100_000), true, 10)
            .await
            .unwrap();

        let resolved = resolve_start_position(&pool, user, item_id, None)
            .await
            .unwrap();
        assert_eq!(resolved, None);
    }

    #[tokio::test]
    async fn no_progress_means_no_resume() {
        let pool = db::test_pool().await;
        let user = seed_user(&pool).await;
        let item_id = seed_item(&pool).await;
        let resolved = resolve_start_position(&pool, user, item_id, None)
            .await
            .unwrap();
        assert_eq!(resolved, None);
    }
}
