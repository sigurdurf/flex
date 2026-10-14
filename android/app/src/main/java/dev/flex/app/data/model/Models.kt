package dev.flex.app.data.model

import kotlinx.serialization.SerialName
import kotlinx.serialization.Serializable

// This file mirrors the wire types defined in crates/flex-protocol/src/lib.rs.
// Wire names are snake_case; Kotlin properties are camelCase via @SerialName.

@Serializable
enum class UserRole {
    @SerialName("admin")
    ADMIN,

    @SerialName("user")
    USER,
}

@Serializable
data class User(
    val id: Long,
    val username: String,
    val role: UserRole,
    @SerialName("created_at") val createdAt: Long,
)

@Serializable
data class LoginRequest(
    val username: String,
    val password: String,
    @SerialName("device_name") val deviceName: String? = null,
)

@Serializable
data class LoginResponse(
    val token: String,
    val user: User,
    @SerialName("expires_at") val expiresAt: Long? = null,
)

@Serializable
data class Device(
    val id: Long,
    val name: String,
    @SerialName("created_at") val createdAt: Long,
    @SerialName("last_seen_at") val lastSeenAt: Long? = null,
    @SerialName("expires_at") val expiresAt: Long? = null,
    val current: Boolean = false,
)

@Serializable
enum class PlaybackMode {
    @SerialName("direct")
    DIRECT,

    @SerialName("remux")
    REMUX,

    @SerialName("transcode")
    TRANSCODE,
}

@Serializable
enum class StreamKind {
    @SerialName("file")
    FILE,

    @SerialName("hls")
    HLS,
}

@Serializable
data class ClientCapabilities(
    val containers: List<String> = emptyList(),
    @SerialName("video_codecs") val videoCodecs: List<String> = emptyList(),
    @SerialName("audio_codecs") val audioCodecs: List<String> = emptyList(),
    @SerialName("max_width") val maxWidth: Int? = null,
    @SerialName("max_height") val maxHeight: Int? = null,
    @SerialName("max_bitrate") val maxBitrate: Long? = null,
    val hls: Boolean = false,
    val hdr: Boolean = false,
)

@Serializable
data class PlaybackStartRequest(
    @SerialName("item_id") val itemId: Long,
    val capabilities: ClientCapabilities,
    @SerialName("audio_track") val audioTrack: Int? = null,
    @SerialName("subtitle_track") val subtitleTrack: Int? = null,
    @SerialName("start_position_ms") val startPositionMs: Long? = null,
)

@Serializable
data class PlaybackSession(
    @SerialName("session_id") val sessionId: String,
    val mode: PlaybackMode,
    @SerialName("stream_kind") val streamKind: StreamKind,
    val url: String,
    val mime: String? = null,
    @SerialName("duration_ms") val durationMs: Long? = null,
    @SerialName("audio_stream_index") val audioStreamIndex: Int? = null,
    @SerialName("subtitle_stream_index") val subtitleStreamIndex: Int? = null,
    @SerialName("session_token") val sessionToken: String,
    @SerialName("expires_at") val expiresAt: Long? = null,
)

@Serializable
data class StopPlaybackRequest(
    @SerialName("session_id") val sessionId: String,
)

@Serializable
enum class LibraryKind {
    @SerialName("movies")
    MOVIES,

    @SerialName("tv")
    TV,

    @SerialName("music")
    MUSIC,

    @SerialName("other")
    OTHER,
}

@Serializable
enum class MediaKind {
    @SerialName("movie")
    MOVIE,

    @SerialName("episode")
    EPISODE,

    @SerialName("other")
    OTHER,
}

@Serializable
data class Library(
    val id: Long,
    val name: String,
    val path: String,
    val kind: LibraryKind,
    @SerialName("created_at") val createdAt: Long,
    @SerialName("item_count") val itemCount: Long,
)

@Serializable
data class VideoStream(
    val codec: String,
    val width: Int,
    val height: Int,
    val fps: Double? = null,
    val bitrate: Long? = null,
)

@Serializable
data class AudioStream(
    val index: Int,
    val codec: String,
    val channels: Int? = null,
    val language: String? = null,
    val title: String? = null,
    val bitrate: Long? = null,
)

@Serializable
data class SubtitleStream(
    val index: Int,
    val codec: String,
    val language: String? = null,
    val title: String? = null,
    val forced: Boolean = false,
    val default: Boolean = false,
)

@Serializable
data class MediaItem(
    val id: Long,
    @SerialName("library_id") val libraryId: Long,
    val kind: MediaKind,
    val title: String,
    @SerialName("sort_title") val sortTitle: String,
    val year: Int? = null,
    val overview: String? = null,
    @SerialName("file_path") val filePath: String,
    @SerialName("file_size") val fileSize: Long,
    val container: String? = null,
    @SerialName("duration_ms") val durationMs: Long? = null,
    val video: VideoStream? = null,
    val audio: List<AudioStream> = emptyList(),
    val subtitles: List<SubtitleStream> = emptyList(),
    @SerialName("added_at") val addedAt: Long,
    @SerialName("updated_at") val updatedAt: Long,
    @SerialName("poster_url") val posterUrl: String? = null,
    @SerialName("backdrop_url") val backdropUrl: String? = null,
)

@Serializable
data class Page<T>(
    val items: List<T>,
    val total: Long,
    val limit: Long,
    val offset: Long,
)

@Serializable
data class ApiError(
    val error: String,
    val message: String,
)
