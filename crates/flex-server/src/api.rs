//! HTTP API: routes, handlers and byte-range streaming.

use std::path::PathBuf;
use std::sync::Arc;

use axum::body::Body;
use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::Response;
use axum::routing::{get, post};
use axum::{Json, Router};
use flex_protocol::{
    Health, ItemQuery, Library, MediaItem, Page, ScanReport, SubtitleStream, API_VERSION,
};
use serde::Deserialize;
use sqlx::SqlitePool;
use tokio::io::{AsyncReadExt, AsyncSeekExt, SeekFrom};
use tokio_util::io::ReaderStream;
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;

use crate::config::Config;
use crate::error::AppError;
use crate::{db, scanner};

/// Shared application state.
#[derive(Clone)]
pub struct AppState {
    pub pool: SqlitePool,
    pub config: Arc<Config>,
}

/// Build the application router.
pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/api/v1/health", get(health))
        .route("/api/v1/libraries", get(list_libraries))
        .route("/api/v1/scan", post(trigger_scan))
        .route("/api/v1/items", get(list_items))
        .route("/api/v1/items/{id}", get(get_item))
        .route("/api/v1/items/{id}/stream", get(stream))
        .route("/api/v1/items/{id}/artwork", get(artwork))
        .route("/api/v1/items/{id}/subtitles", get(list_subtitles))
        .route(
            "/api/v1/items/{id}/subtitles/{track}",
            get(get_subtitle_track),
        )
        .layer(CorsLayer::permissive())
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

async fn health() -> Json<Health> {
    Json(Health {
        status: "ok".to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        api_version: API_VERSION.to_string(),
    })
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

fn image_content_type(path: &str) -> &'static str {
    match path.rsplit('.').next().map(|e| e.to_ascii_lowercase()) {
        Some(ext) if ext == "png" => "image/png",
        Some(ext) if ext == "webp" => "image/webp",
        Some(ext) if ext == "gif" => "image/gif",
        _ => "image/jpeg",
    }
}

/// Open a file and build a streaming body, optionally limited to `len` bytes.
async fn file_body(path: &std::path::Path, start: u64, len: Option<u64>) -> Result<Body, AppError> {
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
    State(state): State<AppState>,
    Path(id): Path<i64>,
    headers: HeaderMap,
) -> Result<Response, AppError> {
    let item = db::get_item(&state.pool, id)
        .await?
        .ok_or_else(|| AppError::not_found(format!("media item {id} not found")))?;

    let path = PathBuf::from(&item.file_path);
    let metadata = tokio::fs::metadata(&path)
        .await
        .map_err(|_| AppError::not_found(format!("file for item {id} is unavailable")))?;
    let size = metadata.len();
    let content_type = content_type_for(item.container.as_deref());

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
