package dev.flex.app.ui

import android.app.Application
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.lifecycle.AndroidViewModel
import androidx.lifecycle.viewModelScope
import dev.flex.app.FlexApp
import dev.flex.app.data.FlexRepository
import dev.flex.app.data.TokenStore
import dev.flex.app.data.model.ContinueWatchingEntry
import dev.flex.app.data.model.Library
import dev.flex.app.data.model.MediaItem
import dev.flex.app.data.model.PlaybackMode
import dev.flex.app.data.model.WatchState
import kotlinx.coroutines.launch

data class PlayerUiState(
    val loading: Boolean = false,
    val url: String? = null,
    val mode: PlaybackMode? = null,
    val sessionId: String? = null,
    val itemId: Long? = null,
    val startPositionMs: Long = 0,
    val error: String? = null,
)

class MainViewModel(application: Application) : AndroidViewModel(application) {

    private val container = (application as FlexApp).container
    private val repository: FlexRepository = container.repository
    private val tokenStore: TokenStore = container.tokenStore

    var isLoggedIn by mutableStateOf(tokenStore.token != null)
        private set

    val serverUrl: String get() = tokenStore.baseUrl

    // --- Login ---------------------------------------------------------------
    var loginLoading by mutableStateOf(false)
        private set
    var loginError by mutableStateOf<String?>(null)
        private set

    fun login(
        baseUrl: String,
        username: String,
        password: String,
        deviceName: String,
        onSuccess: () -> Unit,
    ) {
        if (loginLoading) return
        loginLoading = true
        loginError = null
        viewModelScope.launch {
            runCatching {
                repository.login(baseUrl, username.trim(), password, deviceName)
            }.onSuccess {
                isLoggedIn = true
                loginLoading = false
                onSuccess()
            }.onFailure { error ->
                loginError = error.message ?: "Login failed"
                loginLoading = false
            }
        }
    }

    // --- Libraries / browse --------------------------------------------------
    var libraries by mutableStateOf<List<Library>>(emptyList())
        private set
    var selectedLibraryId by mutableStateOf<Long?>(null)
        private set

    var items by mutableStateOf<List<MediaItem>>(emptyList())
        private set
    var itemsLoading by mutableStateOf(false)
        private set
    var itemsError by mutableStateOf<String?>(null)
        private set
    var query by mutableStateOf("")
        private set
    var total by mutableStateOf(0L)
        private set

    private var offset = 0

    fun loadLibraries() {
        viewModelScope.launch {
            runCatching { repository.libraries() }
                .onSuccess { libraries = it }
        }
    }

    fun selectLibrary(id: Long?) {
        selectedLibraryId = id
        refreshItems()
    }

    fun onSearch(newQuery: String) {
        query = newQuery
        refreshItems()
    }

    fun refreshItems() {
        offset = 0
        items = emptyList()
        loadMoreItems()
    }

    fun loadMoreItems() {
        if (itemsLoading) return
        itemsLoading = true
        itemsError = null
        viewModelScope.launch {
            runCatching {
                repository.items(
                    libraryId = selectedLibraryId,
                    query = query,
                    limit = FlexRepository.PAGE_SIZE,
                    offset = offset,
                )
            }.onSuccess { page ->
                items = items + page.items
                total = page.total
                offset = items.size
            }.onFailure { error ->
                itemsError = error.message ?: "Failed to load items"
            }
            itemsLoading = false
        }
    }

    // --- Continue watching ---------------------------------------------------
    var continueWatching by mutableStateOf<List<ContinueWatchingEntry>>(emptyList())
        private set

    fun loadContinueWatching() {
        viewModelScope.launch {
            runCatching { repository.continueWatching() }
                .onSuccess { continueWatching = it }
        }
    }

    // --- Detail --------------------------------------------------------------
    var detailItem by mutableStateOf<MediaItem?>(null)
        private set
    var detailLoading by mutableStateOf(false)
        private set
    var detailError by mutableStateOf<String?>(null)
        private set
    var detailProgress by mutableStateOf<WatchState?>(null)
        private set

    // Set when resuming from a "Continue watching" card so playback can start
    // without first loading the detail page.
    private var pendingResume by mutableStateOf<WatchState?>(null)

    fun loadDetail(id: Long) {
        detailLoading = true
        detailError = null
        detailItem = null
        detailProgress = null
        pendingResume = null
        viewModelScope.launch {
            runCatching { repository.item(id) }
                .onSuccess { detailItem = it }
                .onFailure { detailError = it.message ?: "Failed to load item" }
            runCatching { repository.progress(id) }
                .onSuccess { detailProgress = it }
            detailLoading = false
        }
    }

    /** Remembers a resume point when launching from a "Continue watching" card. */
    fun setResumeOverride(state: WatchState?) {
        pendingResume = state
    }

    /** Resume position for the next playback, or 0 when there is nothing to resume. */
    fun resumePositionMs(): Long {
        val state = pendingResume ?: detailProgress ?: return 0
        return if (!state.watched && state.positionMs > 0) state.positionMs else 0
    }

    fun artworkUrl(item: MediaItem): String? = repository.artworkUrl(item)

    fun backdropUrl(item: MediaItem): String? = repository.backdropUrl(item)

    // --- Playback ------------------------------------------------------------
    var player by mutableStateOf(PlayerUiState())
        private set

    fun currentToken(): String? = tokenStore.token

    fun startPlayback(itemId: Long, startPositionMs: Long = 0) {
        player = PlayerUiState(loading = true, itemId = itemId, startPositionMs = startPositionMs)
        viewModelScope.launch {
            runCatching { repository.startPlayback(itemId, startPositionMs) }
                .onSuccess { session ->
                    player = PlayerUiState(
                        loading = false,
                        url = repository.absoluteUrl(session.url),
                        mode = session.mode,
                        sessionId = session.sessionId,
                        itemId = itemId,
                        startPositionMs = startPositionMs,
                    )
                }
                .onFailure { error ->
                    player = PlayerUiState(
                        loading = false,
                        itemId = itemId,
                        error = error.message ?: "Playback failed to start",
                    )
                }
        }
    }

    /** Persists the current playback position; safe to call repeatedly during playback. */
    fun saveProgress(positionMs: Long, durationMs: Long?) {
        val itemId = player.itemId ?: return
        viewModelScope.launch { repository.saveProgress(itemId, positionMs, durationMs) }
    }

    fun stopPlayback() {
        val sessionId = player.sessionId
        player = PlayerUiState()
        if (sessionId != null) {
            viewModelScope.launch { repository.stopPlayback(sessionId) }
        }
    }

    /** Clears saved progress for an item and drops it from "Continue watching". */
    fun clearProgress(itemId: Long) {
        viewModelScope.launch {
            repository.clearProgress(itemId)
            if (detailItem?.id == itemId) detailProgress = null
            continueWatching = continueWatching.filterNot { it.item.id == itemId }
        }
    }

    // --- Logout --------------------------------------------------------------
    fun logout(onDone: () -> Unit) {
        viewModelScope.launch {
            repository.logout()
            libraries = emptyList()
            items = emptyList()
            detailItem = null
            detailProgress = null
            continueWatching = emptyList()
            isLoggedIn = false
            onDone()
        }
    }
}
