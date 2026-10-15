package dev.flex.app.data

import dev.flex.app.data.model.ContinueWatchingEntry
import dev.flex.app.data.model.Library
import dev.flex.app.data.model.LoginRequest
import dev.flex.app.data.model.LoginResponse
import dev.flex.app.data.model.MediaItem
import dev.flex.app.data.model.Page
import dev.flex.app.data.model.PlaybackSession
import dev.flex.app.data.model.PlaybackStartRequest
import dev.flex.app.data.model.StopPlaybackRequest
import dev.flex.app.data.model.UpdateProgressRequest
import dev.flex.app.data.model.User
import dev.flex.app.data.model.WatchState
import retrofit2.Response
import retrofit2.http.Body
import retrofit2.http.DELETE
import retrofit2.http.GET
import retrofit2.http.POST
import retrofit2.http.PUT
import retrofit2.http.Path
import retrofit2.http.Query

interface FlexApi {

    @POST("api/v1/auth/login")
    suspend fun login(@Body request: LoginRequest): LoginResponse

    @POST("api/v1/auth/logout")
    suspend fun logout(): Response<Unit>

    @GET("api/v1/auth/me")
    suspend fun me(): User

    @GET("api/v1/libraries")
    suspend fun libraries(): List<Library>

    @GET("api/v1/items")
    suspend fun items(
        @Query("library_id") libraryId: Long? = null,
        @Query("kind") kind: String? = null,
        @Query("q") q: String? = null,
        @Query("sort") sort: String? = null,
        @Query("limit") limit: Long? = null,
        @Query("offset") offset: Long? = null,
    ): Page<MediaItem>

    @GET("api/v1/items/{id}")
    suspend fun item(@Path("id") id: Long): MediaItem

    @POST("api/v1/playback/start")
    suspend fun playbackStart(@Body request: PlaybackStartRequest): PlaybackSession

    @POST("api/v1/playback/stop")
    suspend fun playbackStop(@Body request: StopPlaybackRequest): Response<Unit>

    @GET("api/v1/continue-watching")
    suspend fun continueWatching(
        @Query("limit") limit: Long? = null,
        @Query("offset") offset: Long? = null,
    ): Page<ContinueWatchingEntry>

    @GET("api/v1/progress/{id}")
    suspend fun progress(@Path("id") itemId: Long): Response<WatchState>

    @PUT("api/v1/progress/{id}")
    suspend fun updateProgress(
        @Path("id") itemId: Long,
        @Body request: UpdateProgressRequest,
    ): WatchState

    @DELETE("api/v1/progress/{id}")
    suspend fun deleteProgress(@Path("id") itemId: Long): Response<Unit>
}
