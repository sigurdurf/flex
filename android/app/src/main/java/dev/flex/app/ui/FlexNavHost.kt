package dev.flex.app.ui

import androidx.compose.runtime.Composable
import androidx.lifecycle.viewmodel.compose.viewModel
import androidx.navigation.NavType
import androidx.navigation.compose.NavHost
import androidx.navigation.compose.composable
import androidx.navigation.compose.rememberNavController
import androidx.navigation.navArgument
import dev.flex.app.ui.auth.LoginScreen
import dev.flex.app.ui.detail.DetailScreen
import dev.flex.app.ui.library.LibraryScreen
import dev.flex.app.ui.player.PlayerScreen
import dev.flex.app.ui.series.SeriesScreen

object Routes {
    const val LOGIN = "login"
    const val LIBRARY = "library"
    const val DETAIL = "detail/{id}"
    const val SERIES = "series/{id}"
    const val PLAYER = "player/{id}"

    fun detail(id: Long) = "detail/$id"
    /** Build the navigation route for the series identified by [id]. */
    fun series(id: Long) = "series/$id"
    fun player(id: Long) = "player/$id"
}

/** Host login, library, series, item detail, and playback navigation. */
@Composable
fun FlexNavHost(viewModel: MainViewModel = viewModel()) {
    val navController = rememberNavController()
    val startDestination = if (viewModel.isLoggedIn) Routes.LIBRARY else Routes.LOGIN

    NavHost(navController = navController, startDestination = startDestination) {
        composable(Routes.LOGIN) {
            LoginScreen(viewModel) {
                navController.navigate(Routes.LIBRARY) {
                    popUpTo(Routes.LOGIN) { inclusive = true }
                }
            }
        }

        composable(Routes.LIBRARY) {
            LibraryScreen(
                viewModel = viewModel,
                onItemClick = { id -> navController.navigate(Routes.detail(id)) },
                onSeriesClick = { id -> navController.navigate(Routes.series(id)) },
                onResume = { entry ->
                    viewModel.setResumeOverride(entry.state)
                    navController.navigate(Routes.player(entry.item.id))
                },
                onLogout = {
                    viewModel.logout {
                        navController.navigate(Routes.LOGIN) {
                            popUpTo(Routes.LIBRARY) { inclusive = true }
                        }
                    }
                },
            )
        }

        composable(
            route = Routes.DETAIL,
            arguments = listOf(navArgument("id") { type = NavType.LongType }),
        ) { entry ->
            val id = entry.arguments?.getLong("id") ?: return@composable
            DetailScreen(
                viewModel = viewModel,
                itemId = id,
                onBack = { navController.popBackStack() },
                onPlay = { navController.navigate(Routes.player(id)) },
            )
        }

        composable(
            route = Routes.SERIES,
            arguments = listOf(navArgument("id") { type = NavType.LongType }),
        ) { entry ->
            val id = entry.arguments?.getLong("id") ?: return@composable
            SeriesScreen(
                viewModel = viewModel,
                seriesId = id,
                onBack = { navController.popBackStack() },
                onEpisodeClick = { episodeId -> navController.navigate(Routes.detail(episodeId)) },
            )
        }

        composable(
            route = Routes.PLAYER,
            arguments = listOf(navArgument("id") { type = NavType.LongType }),
        ) { entry ->
            val id = entry.arguments?.getLong("id") ?: return@composable
            PlayerScreen(
                viewModel = viewModel,
                itemId = id,
                onBack = { navController.popBackStack() },
            )
        }
    }
}
