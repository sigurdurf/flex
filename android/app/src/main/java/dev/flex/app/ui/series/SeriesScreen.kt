package dev.flex.app.ui.series

import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.ArrowBack
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Text
import androidx.compose.material3.TopAppBar
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import coil.compose.AsyncImage
import dev.flex.app.data.model.MediaItem
import dev.flex.app.data.model.Season
import dev.flex.app.data.model.Series
import dev.flex.app.ui.MainViewModel
import dev.flex.app.ui.detail.formatDuration

/** Load [seriesId] and show its seasons, loading indicator, or request error. */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun SeriesScreen(
    viewModel: MainViewModel,
    seriesId: Long,
    onBack: () -> Unit,
    onEpisodeClick: (Long) -> Unit,
) {
    LaunchedEffect(seriesId) {
        viewModel.loadSeriesDetail(seriesId)
    }

    Scaffold(
        topBar = {
            TopAppBar(
                title = { Text("Series") },
                navigationIcon = {
                    IconButton(onClick = onBack) {
                        Icon(Icons.AutoMirrored.Filled.ArrowBack, contentDescription = "Back")
                    }
                },
            )
        },
    ) { padding ->
        Box(
            modifier = Modifier
                .fillMaxSize()
                .padding(padding),
        ) {
            val detail = viewModel.seriesDetail
            when {
                viewModel.seriesDetailLoading -> {
                    CircularProgressIndicator(modifier = Modifier.align(Alignment.Center))
                }

                viewModel.seriesDetailError != null -> {
                    Text(
                        text = viewModel.seriesDetailError.orEmpty(),
                        color = MaterialTheme.colorScheme.error,
                        modifier = Modifier
                            .align(Alignment.Center)
                            .padding(24.dp),
                    )
                }

                detail != null -> {
                    SeriesContent(
                        series = detail.series,
                        seasons = detail.seasons,
                        backdropUrl = viewModel.seriesArtworkUrl(detail.series),
                        onEpisodeClick = onEpisodeClick,
                    )
                }
            }
        }
    }
}

/** Render series metadata and season groups whose episodes invoke [onEpisodeClick]. */
@Composable
private fun SeriesContent(
    series: Series,
    seasons: List<Season>,
    backdropUrl: String?,
    onEpisodeClick: (Long) -> Unit,
) {
    LazyColumn(modifier = Modifier.fillMaxSize()) {
        item {
            if (backdropUrl != null) {
                AsyncImage(
                    model = backdropUrl,
                    contentDescription = null,
                    contentScale = ContentScale.Crop,
                    modifier = Modifier
                        .fillMaxWidth()
                        .height(180.dp),
                )
            }

            Column(modifier = Modifier.padding(16.dp)) {
                Text(
                    text = series.title,
                    style = MaterialTheme.typography.headlineSmall,
                )
                val meta = buildList {
                    series.year?.let { add(it.toString()) }
                    add(if (series.seasonCount == 1L) "1 season" else "${series.seasonCount} seasons")
                    add(
                        if (series.episodeCount == 1L) {
                            "1 episode"
                        } else {
                            "${series.episodeCount} episodes"
                        },
                    )
                }.joinToString(" • ")
                Text(
                    text = meta,
                    style = MaterialTheme.typography.bodyMedium,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )

                series.overview?.takeIf { it.isNotBlank() }?.let { overview ->
                    Spacer(Modifier.height(8.dp))
                    Text(
                        text = overview,
                        style = MaterialTheme.typography.bodyMedium,
                    )
                }
            }
        }

        seasons.forEach { season ->
            item(key = "season-${season.seasonNumber}") {
                Text(
                    text = if (season.seasonNumber > 0) {
                        "Season ${season.seasonNumber}"
                    } else {
                        "Specials"
                    },
                    style = MaterialTheme.typography.titleMedium,
                    modifier = Modifier.padding(start = 16.dp, end = 16.dp, top = 12.dp, bottom = 4.dp),
                )
            }
            items(
                count = season.episodes.size,
                key = { index -> season.episodes[index].id },
            ) { index ->
                EpisodeRow(
                    episode = season.episodes[index],
                    onClick = { onEpisodeClick(season.episodes[index].id) },
                )
            }
        }

        item { Spacer(Modifier.height(24.dp)) }
    }
}

/** Render a selectable episode with its number, title, and available duration. */
@Composable
private fun EpisodeRow(episode: MediaItem, onClick: () -> Unit) {
    Column(
        modifier = Modifier
            .fillMaxWidth()
            .clickable(onClick = onClick)
            .padding(horizontal = 16.dp, vertical = 8.dp),
    ) {
        Text(
            text = episodeLabel(episode),
            style = MaterialTheme.typography.bodyMedium,
            maxLines = 1,
            overflow = TextOverflow.Ellipsis,
        )
        val secondary = buildList {
            add(episode.title)
            episode.durationMs?.let { add(formatDuration(it)) }
        }.joinToString(" • ")
        Text(
            text = secondary,
            style = MaterialTheme.typography.bodySmall,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
            maxLines = 1,
            overflow = TextOverflow.Ellipsis,
        )
    }
    HorizontalDivider()
}

/** Format a positive episode number as E01, falling back to Episode otherwise. */
private fun episodeLabel(episode: MediaItem): String {
    val number = episode.episode?.takeIf { it > 0 }
    return number?.let { "E%02d".format(it) } ?: "Episode"
}
