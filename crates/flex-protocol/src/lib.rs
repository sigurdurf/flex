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
