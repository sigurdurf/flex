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

object Routes {
    const val LOGIN = "login"
    const val LIBRARY = "library"
    const val DETAIL = "detail/{id}"
    const val PLAYER = "player/{id}"

    fun detail(id: Long) = "detail/$id"
    fun player(id: Long) = "player/$id"
}

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
