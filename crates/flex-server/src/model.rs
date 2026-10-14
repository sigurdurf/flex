//! Mapping between SQLite rows and `flex-protocol` types.

use flex_protocol::{
    AudioStream, Device, MediaItem, MediaKind, SubtitleStream, User, UserRole, VideoStream,
};
use serde::{Deserialize, Serialize};
use sqlx::sqlite::SqliteRow;
use sqlx::Row;

/// A subtitle file that lives next to the video (`.srt`/`.vtt`).
///
/// Stored in the `sidecars_json` column; not part of the wire protocol. The
/// `index` is synthetic (offset well above any real ffmpeg stream index) so the
/// combined subtitle list can be addressed by a single `u32` track id.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SidecarTrack {
    pub index: u32,
    pub path: String,
    pub codec: String,
    pub language: Option<String>,
    pub title: Option<String>,
    pub forced: bool,
    pub default: bool,
}

/// Base index used for synthetic sidecar tracks.
pub const SIDECAR_INDEX_BASE: u32 = 100_000;

impl SidecarTrack {
    pub fn to_stream(&self) -> SubtitleStream {
        SubtitleStream {
            index: self.index,
            codec: self.codec.clone(),
            language: self.language.clone(),
            title: self.title.clone(),
            forced: self.forced,
            default: self.default,
        }
    }

    /// Whether this sidecar is already WebVTT and can be served verbatim.
    pub fn is_webvtt(&self) -> bool {
        self.codec.eq_ignore_ascii_case("webvtt") || self.codec.eq_ignore_ascii_case("vtt")
    }
}

/// Serialize a [`LibraryKind`] to the lowercase string stored in SQLite.
pub fn library_kind_str(kind: flex_protocol::LibraryKind) -> &'static str {
    use flex_protocol::LibraryKind::*;
    match kind {
        Movies => "movies",
        Tv => "tv",
        Music => "music",
        Other => "other",
    }
}

/// Parse a library kind from its stored string form.
pub fn library_kind_from_str(value: &str) -> flex_protocol::LibraryKind {
    use flex_protocol::LibraryKind::*;
    match value.to_ascii_lowercase().as_str() {
        "movies" => Movies,
        "tv" => Tv,
        "music" => Music,
        _ => Other,
    }
}

/// Serialize a [`UserRole`] to the lowercase string stored in SQLite.
pub fn role_str(role: UserRole) -> &'static str {
    match role {
        UserRole::Admin => "admin",
        UserRole::User => "user",
    }
}

/// Parse a user role from its stored string form.
pub fn role_from_str(value: &str) -> UserRole {
    match value.to_ascii_lowercase().as_str() {
        "admin" => UserRole::Admin,
        _ => UserRole::User,
    }
}

/// Decode a `users` row into a [`User`] (never exposes the password hash).
pub fn row_to_user(row: &SqliteRow) -> Result<User, sqlx::Error> {
    let role: String = row.try_get("role")?;
    Ok(User {
        id: row.try_get("id")?,
        username: row.try_get("username")?,
        role: role_from_str(&role),
        created_at: row.try_get("created_at")?,
    })
}

/// Decode a `devices` row into a [`Device`].
///
/// `current` defaults to `false`; callers mark the requesting token.
pub fn row_to_device(row: &SqliteRow) -> Result<Device, sqlx::Error> {
    Ok(Device {
        id: row.try_get("id")?,
        name: row.try_get("name")?,
        created_at: row.try_get("created_at")?,
        last_seen_at: row.try_get("last_seen_at")?,
        expires_at: row.try_get("expires_at")?,
        current: false,
    })
}

/// Serialize a [`MediaKind`] to the lowercase string stored in SQLite.
pub fn media_kind_str(kind: MediaKind) -> &'static str {
    match kind {
        MediaKind::Movie => "movie",
        MediaKind::Episode => "episode",
        MediaKind::Other => "other",
    }
}

/// Parse a media kind from its stored string form.
pub fn media_kind_from_str(value: &str) -> MediaKind {
    match value.to_ascii_lowercase().as_str() {
        "movie" => MediaKind::Movie,
        "episode" => MediaKind::Episode,
        _ => MediaKind::Other,
    }
}

/// The parsed stream metadata ready to be persisted.
#[derive(Debug, Clone, Default)]
pub struct StreamData {
    pub video: Option<VideoStream>,
    pub audio: Vec<AudioStream>,
    pub subtitles: Vec<SubtitleStream>,
}

/// All fields required to insert/update a media item row.
#[derive(Debug, Clone)]
pub struct NewMediaItem {
    pub library_id: i64,
    pub kind: MediaKind,
    pub title: String,
    pub sort_title: String,
    pub year: Option<i32>,
    pub overview: Option<String>,
    pub file_path: String,
    pub file_size: u64,
    pub container: Option<String>,
    pub duration_ms: Option<u64>,
    pub streams: StreamData,
    pub sidecars: Vec<SidecarTrack>,
    pub poster_path: Option<String>,
    pub backdrop_path: Option<String>,
}

/// Decode a `media_items` row into a [`MediaItem`].
pub fn row_to_media_item(row: &SqliteRow) -> Result<MediaItem, sqlx::Error> {
    let id: i64 = row.try_get("id")?;
    let kind_str: String = row.try_get("kind")?;
    let video_json: Option<String> = row.try_get("video_json")?;
    let audio_json: Option<String> = row.try_get("audio_json")?;
    let subs_json: Option<String> = row.try_get("subs_json")?;
    let sidecars_json: Option<String> = row.try_get("sidecars_json")?;
    let poster_path: Option<String> = row.try_get("poster_path")?;
    let backdrop_path: Option<String> = row.try_get("backdrop_path")?;

    let video: Option<VideoStream> = video_json.and_then(|s| serde_json::from_str(&s).ok());
    let audio: Vec<AudioStream> = audio_json
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();
    let mut subtitles: Vec<SubtitleStream> = subs_json
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();
    let sidecars: Vec<SidecarTrack> = sidecars_json
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();
    subtitles.extend(sidecars.iter().map(SidecarTrack::to_stream));

    Ok(MediaItem {
        id,
        library_id: row.try_get("library_id")?,
        kind: media_kind_from_str(&kind_str),
        title: row.try_get("title")?,
        sort_title: row.try_get("sort_title")?,
        year: row.try_get("year")?,
        overview: row.try_get("overview")?,
        file_path: row.try_get("file_path")?,
        file_size: row.try_get::<i64, _>("file_size")?.max(0) as u64,
        container: row.try_get("container")?,
        duration_ms: row
            .try_get::<Option<i64>, _>("duration_ms")?
            .map(|v| v.max(0) as u64),
        video,
        audio,
        subtitles,
        added_at: row.try_get("added_at")?,
        updated_at: row.try_get("updated_at")?,
        poster_url: poster_path.map(|_| flex_protocol::artwork_path(id) + "?type=poster"),
        backdrop_url: backdrop_path.map(|_| flex_protocol::artwork_path(id) + "?type=backdrop"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use flex_protocol::LibraryKind;

    #[test]
    fn kind_round_trips() {
        for (kind, s) in [
            (LibraryKind::Movies, "movies"),
            (LibraryKind::Tv, "tv"),
            (LibraryKind::Music, "music"),
            (LibraryKind::Other, "other"),
        ] {
            assert_eq!(library_kind_str(kind), s);
            assert_eq!(library_kind_from_str(s), kind);
        }
        assert_eq!(media_kind_str(MediaKind::Movie), "movie");
        assert_eq!(media_kind_from_str("episode"), MediaKind::Episode);
        assert_eq!(media_kind_from_str("bogus"), MediaKind::Other);
    }
}
