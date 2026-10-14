//! # flex-protocol
//!
//! Shared, transport-agnostic data types and constants for the Flex media
//! ecosystem. Both `flex-server` (the headless media server) and `flex-client`
//! (the Linux desktop player) depend on this crate so the wire format stays in
//! sync by construction.
//!
//! The types here define the JSON payloads of the Flex HTTP API. See
//! `docs/API.md` for the endpoint-level contract.

use serde::{Deserialize, Serialize};

/// Version of the HTTP API. Appears in every route as `/api/<API_VERSION>/...`.
pub const API_VERSION: &str = "v1";

/// Default TCP port used by the headless server.
pub const DEFAULT_PORT: u16 = 8096;

/// Default address the server binds to when unset.
pub const DEFAULT_HOST: &str = "0.0.0.0";

/// URL of the health endpoint.
pub fn health_path() -> String {
    format!("/api/{API_VERSION}/health")
}

/// URL listing configured libraries.
pub fn libraries_path() -> String {
    format!("/api/{API_VERSION}/libraries")
}

/// URL to trigger a library scan.
pub fn scan_path() -> String {
    format!("/api/{API_VERSION}/scan")
}

/// URL to browse media items.
pub fn items_path() -> String {
    format!("/api/{API_VERSION}/items")
}

/// URL for a single media item's metadata.
pub fn item_path(id: i64) -> String {
    format!("/api/{API_VERSION}/items/{id}")
}

/// URL for a range-capable byte stream of a media item.
pub fn stream_path(id: i64) -> String {
    format!("/api/{API_VERSION}/items/{id}/stream")
}

/// URL for a media item's artwork (`poster` or `backdrop`).
pub fn artwork_path(id: i64) -> String {
    format!("/api/{API_VERSION}/items/{id}/artwork")
}

/// URL listing a media item's subtitle tracks.
pub fn subtitles_path(id: i64) -> String {
    format!("/api/{API_VERSION}/items/{id}/subtitles")
}

/// URL for a single embedded/external subtitle track.
pub fn subtitle_track_path(id: i64, track: u32) -> String {
    format!("/api/{API_VERSION}/items/{id}/subtitles/{track}")
}

// --- Authentication ---------------------------------------------------------

/// Exchange credentials for a device token.
pub fn login_path() -> String {
    format!("/api/{API_VERSION}/auth/login")
}

/// Invalidate the current device token.
pub fn logout_path() -> String {
    format!("/api/{API_VERSION}/auth/logout")
}

/// Fetch the authenticated user.
pub fn me_path() -> String {
    format!("/api/{API_VERSION}/auth/me")
}

/// List the authenticated user's device tokens.
pub fn devices_path() -> String {
    format!("/api/{API_VERSION}/auth/devices")
}

/// Revoke a specific device token.
pub fn device_path(id: i64) -> String {
    format!("/api/{API_VERSION}/auth/devices/{id}")
}

// --- Playback ---------------------------------------------------------------

/// Begin a playback session, choosing direct play / remux / transcode.
pub fn playback_start_path() -> String {
    format!("/api/{API_VERSION}/playback/start")
}

/// Stop a playback session and free any transcode resources.
pub fn playback_stop_path() -> String {
    format!("/api/{API_VERSION}/playback/stop")
}

/// HLS master playlist for a playback session.
pub fn playback_master_path(session_id: &str) -> String {
    format!("/api/{API_VERSION}/playback/{session_id}/master.m3u8")
}

/// A file (playlist / segment / init) belonging to a playback session.
pub fn playback_file_path(session_id: &str, file: &str) -> String {
    format!("/api/{API_VERSION}/playback/{session_id}/{file}")
}

/// Server capabilities / build info.
pub fn system_info_path() -> String {
    format!("/api/{API_VERSION}/system/info")
}

/// A user account role.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum UserRole {
    Admin,
    #[default]
    User,
}

/// A user account (never carries the password hash over the wire).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct User {
    pub id: i64,
    pub username: String,
    pub role: UserRole,
    pub created_at: i64,
}

/// Credentials plus an optional human-readable device name.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoginRequest {
    pub username: String,
    pub password: String,
    pub device_name: Option<String>,
}

/// A freshly issued device token.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoginResponse {
    /// Opaque bearer token. Send as `Authorization: Bearer <token>`.
    pub token: String,
    pub user: User,
    /// Unix time (seconds) the token expires, or `None` for non-expiring.
    pub expires_at: Option<i64>,
}

/// Metadata about an issued device token, for revocation UIs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Device {
    pub id: i64,
    pub name: String,
    pub created_at: i64,
    pub last_seen_at: Option<i64>,
    pub expires_at: Option<i64>,
    /// True for the token making the current request.
    pub current: bool,
}

/// Request body for admin user creation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateUserRequest {
    pub username: String,
    pub password: String,
    #[serde(default)]
    pub admin: bool,
}

/// What the server chose to do for a playback request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PlaybackMode {
    /// The original file is served untouched.
    Direct,
    /// Streams are copied into a supported container (no video re-encode).
    Remux,
    /// Video (and possibly audio) is re-encoded.
    Transcode,
}

/// How the client should consume a playback session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StreamKind {
    /// A single byte-range capable file (see [`stream_path`]).
    File,
    /// An HLS playlist.
    Hls,
}

/// Decoder/container support advertised by a client.
///
/// Codec names are matched case-insensitively against ffprobe `codec_name`
/// values (e.g. `h264`, `hevc`, `aac`, `eac3`). Container names match the first
/// comma-separated token of ffprobe's `format_name` (e.g. `matroska`, `mov`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClientCapabilities {
    #[serde(default)]
    pub containers: Vec<String>,
    #[serde(default)]
    pub video_codecs: Vec<String>,
    #[serde(default)]
    pub audio_codecs: Vec<String>,
    /// Maximum decodable width, if bounded.
    #[serde(default)]
    pub max_width: Option<u32>,
    /// Maximum decodable height, if bounded.
    #[serde(default)]
    pub max_height: Option<u32>,
    /// Maximum sustainable bitrate in bits/second.
    #[serde(default)]
    pub max_bitrate: Option<u64>,
    /// Whether the client can play HLS.
    #[serde(default)]
    pub hls: bool,
    /// Whether the client can render HDR.
    #[serde(default)]
    pub hdr: bool,
}

/// Request to start playback.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlaybackStartRequest {
    pub item_id: i64,
    pub capabilities: ClientCapabilities,
    /// Preferred audio stream index (as reported by `MediaItem.audio`).
    #[serde(default)]
    pub audio_track: Option<u32>,
    /// Preferred subtitle stream index (as reported by `MediaItem.subtitles`).
    #[serde(default)]
    pub subtitle_track: Option<u32>,
    /// Resume position in milliseconds.
    #[serde(default)]
    pub start_position_ms: Option<u64>,
}

/// A live playback session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlaybackSession {
    pub session_id: String,
    pub mode: PlaybackMode,
    pub stream_kind: StreamKind,
    /// URL to hand to the player. For HLS this is the master playlist; the URL
    /// already carries a scoped `t=<session_token>` query parameter so segment
    /// requests need no extra headers.
    pub url: String,
    pub mime: Option<String>,
    pub duration_ms: Option<u64>,
    pub audio_stream_index: Option<u32>,
    pub subtitle_stream_index: Option<u32>,
    /// Ephemeral token embedded in `url`; authorises only this session.
    pub session_token: String,
    /// Unix time (seconds) after which the session is reclaimed.
    pub expires_at: Option<i64>,
}

/// Request to stop playback.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StopPlaybackRequest {
    pub session_id: String,
}

/// Server build + capability information.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemInfo {
    pub version: String,
    pub api_version: String,
    /// Hardware acceleration methods reported by `ffmpeg -hwaccels`.
    pub hwaccels: Vec<String>,
    /// Hardware encoder names available (e.g. `h264_nvenc`).
    pub hw_encoders: Vec<String>,
    /// Whether transcoding is enabled at all.
    pub transcode_enabled: bool,
}

/// The category of content a library holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LibraryKind {
    Movies,
    Tv,
    Music,
    #[default]
    Other,
}

/// The type of a single playable media item.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MediaKind {
    Movie,
    Episode,
    #[default]
    Other,
}

/// A configured media library (a root directory on disk).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Library {
    pub id: i64,
    pub name: String,
    /// Absolute filesystem path that was scanned. Server-local only.
    pub path: String,
    pub kind: LibraryKind,
    /// Unix timestamp (seconds) the library was registered.
    pub created_at: i64,
    /// Number of media items currently indexed in this library.
    pub item_count: i64,
}

/// Video stream details extracted with `ffprobe`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VideoStream {
    pub codec: String,
    pub width: u32,
    pub height: u32,
    /// Frames per second, if a rational frame rate was reported.
    pub fps: Option<f64>,
    pub bitrate: Option<u64>,
}

/// A single audio stream inside a container.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AudioStream {
    pub index: u32,
    pub codec: String,
    pub channels: Option<u32>,
    pub language: Option<String>,
    pub title: Option<String>,
    pub bitrate: Option<u64>,
}

/// A subtitle stream inside a container.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubtitleStream {
    pub index: u32,
    pub codec: String,
    pub language: Option<String>,
    pub title: Option<String>,
    pub forced: bool,
    pub default: bool,
}

/// A single indexed piece of media.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MediaItem {
    pub id: i64,
    pub library_id: i64,
    pub kind: MediaKind,
    pub title: String,
    /// Title normalised for sorting (articles stripped, lowercased server-side).
    pub sort_title: String,
    pub year: Option<i32>,
    pub overview: Option<String>,
    /// Absolute server-local path to the file. Stream clients should use
    /// [`stream_path`] instead.
    pub file_path: String,
    pub file_size: u64,
    pub container: Option<String>,
    pub duration_ms: Option<u64>,
    pub video: Option<VideoStream>,
    pub audio: Vec<AudioStream>,
    pub subtitles: Vec<SubtitleStream>,
    pub added_at: i64,
    pub updated_at: i64,
    /// Present when artwork is available. Relative to the API root.
    pub poster_url: Option<String>,
    pub backdrop_url: Option<String>,
}

/// A page of results returned by list endpoints.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Page<T> {
    pub items: Vec<T>,
    pub total: i64,
    pub limit: i64,
    pub offset: i64,
}

/// Response of `GET /api/v1/health`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Health {
    pub status: String,
    pub version: String,
    pub api_version: String,
}

/// Response of `POST /api/v1/scan`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ScanReport {
    pub libraries: usize,
    pub scanned: usize,
    pub added: usize,
    pub updated: usize,
    pub removed: usize,
    pub errors: Vec<String>,
}

/// Query parameters accepted by `GET /api/v1/items`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ItemQuery {
    pub library_id: Option<i64>,
    pub kind: Option<MediaKind>,
    /// Fuzzy title search.
    pub q: Option<String>,
    /// Sort key: `title`, `-title`, `year`, `-year`, `added`, `-added`.
    pub sort: Option<String>,
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

/// A normalised error body returned by the API on failure.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiError {
    pub error: String,
    pub message: String,
}
