package dev.flex.app.ui.library

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.LazyRow
import androidx.compose.foundation.lazy.grid.GridCells
import androidx.compose.foundation.lazy.grid.LazyVerticalGrid
import androidx.compose.foundation.lazy.grid.items
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.Logout
import androidx.compose.material.icons.filled.Movie
import androidx.compose.material.icons.filled.Search
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
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
import androidx.compose.foundation.shape.RoundedCornerShape
import coil.compose.AsyncImage
import dev.flex.app.data.model.ContinueWatchingEntry
import dev.flex.app.data.model.Library
import dev.flex.app.data.model.MediaItem
import dev.flex.app.ui.MainViewModel

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun LibraryScreen(
    viewModel: MainViewModel,
    onItemClick: (Long) -> Unit,
    onResume: (ContinueWatchingEntry) -> Unit,
    onLogout: () -> Unit,
) {
    LaunchedEffect(Unit) {
        viewModel.loadLibraries()
        viewModel.loadContinueWatching()
        viewModel.refreshItems()
    }

    Scaffold(
        topBar = {
            TopAppBar(
                title = { Text("Flex") },
                actions = {
                    IconButton(onClick = onLogout) {
                        Icon(Icons.AutoMirrored.Filled.Logout, contentDescription = "Log out")
                    }
                },
            )
        },
    ) { padding ->
        Column(
            modifier = Modifier
                .fillMaxSize()
                .padding(padding),
        ) {
            OutlinedTextField(
                value = viewModel.query,
                onValueChange = viewModel::onSearch,
                leadingIcon = { Icon(Icons.Filled.Search, contentDescription = null) },
                placeholder = { Text("Search titles") },
                singleLine = true,
                modifier = Modifier
                    .fillMaxWidth()
                    .padding(horizontal = 12.dp, vertical = 8.dp),
            )

            if (viewModel.query.isBlank() && viewModel.continueWatching.isNotEmpty()) {
                ContinueWatchingRow(
                    entries = viewModel.continueWatching,
                    artworkFor = viewModel::artworkUrl,
                    onResume = onResume,
                )
            }

            Row(modifier = Modifier.fillMaxSize()) {
                LibrarySidebar(
                    libraries = viewModel.libraries,
                    selectedId = viewModel.selectedLibraryId,
                    onSelect = viewModel::selectLibrary,
                    modifier = Modifier
                        .width(140.dp)
                        .fillMaxHeight(),
                )

                Box(modifier = Modifier.fillMaxSize()) {
                    ItemGrid(
                        items = viewModel.items,
                        artworkFor = viewModel::artworkUrl,
                        onItemClick = onItemClick,
                        onLoadMore = viewModel::loadMoreItems,
                        hasMore = viewModel.items.size.toLong() < viewModel.total,
                        modifier = Modifier.fillMaxSize(),
                    )

                    if (viewModel.itemsLoading && viewModel.items.isEmpty()) {
                        CircularProgressIndicator(modifier = Modifier.align(Alignment.Center))
                    }

                    viewModel.itemsError?.let { error ->
                        Text(
                            text = error,
                            color = MaterialTheme.colorScheme.error,
                            modifier = Modifier
                                .align(Alignment.Center)
                                .padding(24.dp),
                        )
                    }
                }
            }
        }
    }
}

@Composable
private fun LibrarySidebar(
    libraries: List<Library>,
    selectedId: Long?,
    onSelect: (Long?) -> Unit,
    modifier: Modifier = Modifier,
) {
    Column(
        modifier = modifier
            .background(MaterialTheme.colorScheme.surface)
            .padding(vertical = 8.dp),
    ) {
        SidebarEntry(
            label = "All",
            selected = selectedId == null,
            onClick = { onSelect(null) },
        )
        LazyColumn(modifier = Modifier.fillMaxWidth()) {
            items(
                count = libraries.size,
                key = { index -> libraries[index].id },
            ) { index ->
                val library = libraries[index]
                SidebarEntry(
                    label = library.name,
                    selected = selectedId == library.id,
                    onClick = { onSelect(library.id) },
                )
            }
        }
    }
}

@Composable
private fun SidebarEntry(
    label: String,
    selected: Boolean,
    onClick: () -> Unit,
) {
    val background = if (selected) {
        MaterialTheme.colorScheme.primaryContainer
    } else {
        MaterialTheme.colorScheme.surface
    }
    Text(
        text = label,
        maxLines = 2,
        overflow = TextOverflow.Ellipsis,
        style = MaterialTheme.typography.bodyMedium,
        modifier = Modifier
            .fillMaxWidth()
            .clickable(onClick = onClick)
            .background(background)
            .padding(horizontal = 12.dp, vertical = 12.dp),
    )
}

@Composable
private fun ContinueWatchingRow(
    entries: List<ContinueWatchingEntry>,
    artworkFor: (MediaItem) -> String?,
    onResume: (ContinueWatchingEntry) -> Unit,
) {
    Column(modifier = Modifier.fillMaxWidth()) {
        Text(
            text = "Continue watching",
            style = MaterialTheme.typography.titleMedium,
            modifier = Modifier.padding(start = 12.dp, top = 4.dp, bottom = 4.dp),
        )
        LazyRow(
            contentPadding = androidx.compose.foundation.layout.PaddingValues(horizontal = 12.dp),
            horizontalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            items(
                count = entries.size,
                key = { index -> entries[index].item.id },
            ) { index ->
                ContinueWatchingCard(
                    entry = entries[index],
                    artworkUrl = artworkFor(entries[index].item),
                    onClick = { onResume(entries[index]) },
                )
            }
        }
    }
}

@Composable
private fun ContinueWatchingCard(
    entry: ContinueWatchingEntry,
    artworkUrl: String?,
    onClick: () -> Unit,
) {
    val duration = entry.state.durationMs?.takeIf { it > 0 }
    val fraction = duration?.let {
        (entry.state.positionMs.toDouble() / it.toDouble()).coerceIn(0.0, 1.0).toFloat()
    }

    Column(
        modifier = Modifier
            .width(140.dp)
            .clickable(onClick = onClick),
    ) {
        Box(
            modifier = Modifier
                .fillMaxWidth()
                .aspectRatio(2f / 3f)
                .clip(RoundedCornerShape(8.dp))
                .background(MaterialTheme.colorScheme.surfaceVariant),
            contentAlignment = Alignment.Center,
        ) {
            if (artworkUrl != null) {
                AsyncImage(
                    model = artworkUrl,
                    contentDescription = entry.item.title,
                    contentScale = ContentScale.Crop,
                    modifier = Modifier.fillMaxSize(),
                )
            } else {
                Icon(
                    imageVector = Icons.Filled.Movie,
                    contentDescription = null,
                    tint = MaterialTheme.colorScheme.onSurfaceVariant,
                )
            }
        }
        if (fraction != null && fraction > 0f) {
            LinearProgressIndicator(
                progress = { fraction },
                modifier = Modifier
                    .fillMaxWidth()
                    .height(3.dp),
            )
        }
        Spacer(Modifier.height(4.dp))
        Text(
            text = entry.item.title,
            maxLines = 2,
            overflow = TextOverflow.Ellipsis,
            style = MaterialTheme.typography.bodySmall,
        )
    }
}

@Composable
private fun ItemGrid(
    items: List<MediaItem>,
    artworkFor: (MediaItem) -> String?,
    onItemClick: (Long) -> Unit,
    onLoadMore: () -> Unit,
    hasMore: Boolean,
    modifier: Modifier = Modifier,
) {
    LazyVerticalGrid(
        columns = GridCells.Adaptive(minSize = 110.dp),
        modifier = modifier,
        contentPadding = androidx.compose.foundation.layout.PaddingValues(8.dp),
        verticalArrangement = Arrangement.spacedBy(8.dp),
        horizontalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        items(items, key = { it.id }) { item ->
            PosterCard(
                title = item.title,
                year = item.year,
                artworkUrl = artworkFor(item),
                onClick = { onItemClick(item.id) },
            )
        }

        if (hasMore) {
            item {
                Box(
                    modifier = Modifier
                        .fillMaxWidth()
                        .padding(16.dp),
                    contentAlignment = Alignment.Center,
                ) {
                    Text(
                        text = "Load more",
                        color = MaterialTheme.colorScheme.primary,
                        modifier = Modifier.clickable(onClick = onLoadMore),
                    )
                }
            }
        }
    }
}

@Composable
private fun PosterCard(
    title: String,
    year: Int?,
    artworkUrl: String?,
    onClick: () -> Unit,
) {
    Column(
        modifier = Modifier
            .fillMaxWidth()
            .clickable(onClick = onClick),
    ) {
        Box(
            modifier = Modifier
                .fillMaxWidth()
                .aspectRatio(2f / 3f)
                .clip(RoundedCornerShape(8.dp))
                .background(MaterialTheme.colorScheme.surfaceVariant),
            contentAlignment = Alignment.Center,
        ) {
            if (artworkUrl != null) {
                AsyncImage(
                    model = artworkUrl,
                    contentDescription = title,
                    contentScale = ContentScale.Crop,
                    modifier = Modifier.fillMaxSize(),
                )
            } else {
                Icon(
                    imageVector = Icons.Filled.Movie,
                    contentDescription = null,
                    tint = MaterialTheme.colorScheme.onSurfaceVariant,
                )
            }
        }
        Spacer(Modifier.height(4.dp))
        Text(
            text = title,
            maxLines = 2,
            overflow = TextOverflow.Ellipsis,
            style = MaterialTheme.typography.bodySmall,
        )
        if (year != null) {
            Text(
                text = year.toString(),
                style = MaterialTheme.typography.labelSmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
        }
    }
}
