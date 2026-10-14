package dev.flex.app

import android.app.Application
import dev.flex.app.data.AppContainer

class FlexApp : Application() {

    lateinit var container: AppContainer
        private set

    override fun onCreate() {
        super.onCreate()
        container = AppContainer(this)
    }
}
