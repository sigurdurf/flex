package dev.flex.app.data

import dev.flex.app.data.model.ClientCapabilities
import dev.flex.app.data.model.Library
import dev.flex.app.data.model.LoginRequest
import dev.flex.app.data.model.MediaItem
import dev.flex.app.data.model.Page
import dev.flex.app.data.model.PlaybackSession
import dev.flex.app.data.model.PlaybackStartRequest
import dev.flex.app.data.model.StopPlaybackRequest
import dev.flex.app.data.model.User

class FlexRepository(
    private val tokenStore: TokenStore,
    private val apiProvider: () -> FlexApi,
) {

    private val api: FlexApi get() = apiProvider()

    suspend fun login(
        baseUrl: String,
        username: String,
        password: String,
        deviceName: String?,
    ): User {
        tokenStore.baseUrl = baseUrl
        val response = api.login(
            LoginRequest(
                username = username,
                password = password,
                deviceName = deviceName?.takeIf { it.isNotBlank() },
            ),
        )
        tokenStore.saveSession(response.token, response.user)
        return response.user
    }

    suspend fun logout() {
        runCatching { api.logout() }
        tokenStore.clear()
    }

    suspend fun libraries(): List<Library> = api.libraries()

    suspend fun items(
        libraryId: Long?,
        query: String?,
        limit: Int = PAGE_SIZE,
        offset: Int = 0,
    ): Page<MediaItem> = api.items(
        libraryId = libraryId,
        q = query?.takeIf { it.isNotBlank() },
        sort = "sort_title",
        limit = limit.toLong(),
        offset = offset.toLong(),
    )

    suspend fun item(id: Long): MediaItem = api.item(id)

    suspend fun startPlayback(itemId: Long): PlaybackSession =
        api.playbackStart(
            PlaybackStartRequest(
                itemId = itemId,
                capabilities = androidCapabilities(),
            ),
        )

    suspend fun stopPlayback(sessionId: String) {
        runCatching { api.playbackStop(StopPlaybackRequest(sessionId)) }
    }

    fun artworkUrl(item: MediaItem): String? = item.posterUrl?.let(::absoluteUrl)

    fun backdropUrl(item: MediaItem): String? = item.backdropUrl?.let(::absoluteUrl)

    /** Turns an API-relative path (e.g. `/api/v1/items/7/artwork`) into an absolute URL. */
    fun absoluteUrl(relative: String): String {
        if (relative.startsWith("http://") || relative.startsWith("https://")) return relative
        val base = tokenStore.baseUrl.trimEnd('/')
        return base + "/" + relative.trimStart('/')
    }

    companion object {
        const val PAGE_SIZE = 50

        /**
         * Advertises what ExoPlayer on Android can typically decode. The server
         * picks direct play / remux / transcode from this.
         */
        fun androidCapabilities(): ClientCapabilities = ClientCapabilities(
            containers = listOf("mp4", "matroska", "webm"),
            videoCodecs = listOf("h264", "hevc", "vp9", "av1"),
            audioCodecs = listOf("aac", "opus", "flac", "mp3", "vorbis", "ac3", "eac3"),
            maxWidth = 3840,
            maxHeight = 2160,
            maxBitrate = 120_000_000,
            hls = true,
            hdr = true,
        )
    }
}
