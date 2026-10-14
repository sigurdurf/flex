package dev.flex.app.data

import android.content.Context

/**
 * Simple manual DI container owned by [dev.flex.app.FlexApp].
 *
 * Rebuilds the Retrofit-backed [FlexApi] whenever the configured base URL
 * changes so that a server URL edited on the login screen takes effect.
 */
class AppContainer(context: Context) {

    val tokenStore = TokenStore(context.applicationContext)

    private var cachedBaseUrl: String? = null
    private var cachedApi: FlexApi? = null

    @Synchronized
    private fun api(): FlexApi {
        val base = tokenStore.baseUrl
        val current = cachedApi
        if (current == null || cachedBaseUrl != base) {
            val created = ApiClient.create(base, tokenProvider = { tokenStore.token })
            cachedApi = created
            cachedBaseUrl = base
            return created
        }
        return current
    }

    val repository: FlexRepository = FlexRepository(tokenStore) { api() }
}
