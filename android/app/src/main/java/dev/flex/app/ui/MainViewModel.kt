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
import dev.flex.app.data.model.Library
import dev.flex.app.data.model.MediaItem
import dev.flex.app.data.model.PlaybackMode
import kotlinx.coroutines.launch

data class PlayerUiState(
    val loading: Boolean = false,
    val url: String? = null,
    val mode: PlaybackMode? = null,
    val sessionId: String? = null,
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

    // --- Detail --------------------------------------------------------------
    var detailItem by mutableStateOf<MediaItem?>(null)
        private set
    var detailLoading by mutableStateOf(false)
        private set
    var detailError by mutableStateOf<String?>(null)
        private set

    fun loadDetail(id: Long) {
        detailLoading = true
        detailError = null
        detailItem = null
        viewModelScope.launch {
            runCatching { repository.item(id) }
                .onSuccess { detailItem = it }
                .onFailure { detailError = it.message ?: "Failed to load item" }
            detailLoading = false
        }
    }

    fun artworkUrl(item: MediaItem): String? = repository.artworkUrl(item)

    fun backdropUrl(item: MediaItem): String? = repository.backdropUrl(item)

    // --- Playback ------------------------------------------------------------
    var player by mutableStateOf(PlayerUiState())
        private set

    fun currentToken(): String? = tokenStore.token

    fun startPlayback(itemId: Long) {
        player = PlayerUiState(loading = true)
        viewModelScope.launch {
            runCatching { repository.startPlayback(itemId) }
                .onSuccess { session ->
                    player = PlayerUiState(
                        loading = false,
                        url = repository.absoluteUrl(session.url),
                        mode = session.mode,
                        sessionId = session.sessionId,
                    )
                }
                .onFailure { error ->
                    player = PlayerUiState(
                        loading = false,
                        error = error.message ?: "Playback failed to start",
                    )
                }
        }
    }

    fun stopPlayback() {
        val sessionId = player.sessionId
        player = PlayerUiState()
        if (sessionId != null) {
            viewModelScope.launch { repository.stopPlayback(sessionId) }
        }
    }

    // --- Logout --------------------------------------------------------------
    fun logout(onDone: () -> Unit) {
        viewModelScope.launch {
            repository.logout()
            libraries = emptyList()
            items = emptyList()
            detailItem = null
            isLoggedIn = false
            onDone()
        }
    }
}
