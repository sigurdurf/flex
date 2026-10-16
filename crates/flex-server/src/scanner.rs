//! Library scanning: walking directories, probing files and persisting results.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use flex_protocol::{LibraryKind, MediaKind, ScanReport};
use futures::stream::{self, StreamExt};
use sqlx::SqlitePool;
use walkdir::WalkDir;

use crate::config::{Config, LibraryConfig};
use crate::model::{NewMediaItem, NewSeries, SidecarTrack, SIDECAR_INDEX_BASE};
use crate::probe;
use crate::{db, probe::Probed};

/// Number of files probed concurrently.
const PROBE_CONCURRENCY: usize = 8;

/// Normalise whitespace: split on runs of whitespace and re-join with single spaces.
fn collapse_whitespace(input: &str) -> String {
    input.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Find a `(YYYY)` group, returning `(byte_start, byte_end_exclusive, year)`.
fn find_parenthesised_year(input: &str) -> Option<(usize, usize, i32)> {
    let bytes = input.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'(' {
            if let Some(rel) = input[i + 1..].find(')') {
                let end = i + 1 + rel + 1;
                let inner = &input[i + 1..i + 1 + rel];
                if inner.len() == 4 && inner.bytes().all(|b| b.is_ascii_digit()) {
                    if let Ok(year) = inner.parse::<i32>() {
                        if (1000..=2999).contains(&year) {
                            return Some((i, end, year));
                        }
                    }
                }
                i = end;
                continue;
            }
        }
        i += 1;
    }
    None
}

/// Derive a display title and optional year from a file name (without extension).
pub fn parse_title_and_year(stem: &str) -> (String, Option<i32>) {
    let replaced: String = stem
        .chars()
        .map(|c| if c == '.' || c == '_' { ' ' } else { c })
        .collect();
    let mut title = collapse_whitespace(&replaced);
    let mut year = None;

    if let Some((start, end, found)) = find_parenthesised_year(&title) {
        year = Some(found);
        let mut rebuilt = String::with_capacity(title.len());
        rebuilt.push_str(&title[..start]);
        rebuilt.push_str(&title[end..]);
        title = collapse_whitespace(&rebuilt);
    } else if let Some(last) = title.split_whitespace().last() {
        if last.len() == 4 && last.bytes().all(|b| b.is_ascii_digit()) {
            if let Ok(found) = last.parse::<i32>() {
                if (1000..=2999).contains(&found) {
                    year = Some(found);
                    if let Some(idx) = title.rfind(last) {
                        title = title[..idx].trim_end().to_string();
                    }
                }
            }
        }
    }

    (title, year)
}

/// Compute a sort key: lowercased with a leading English article removed.
pub fn sort_title(title: &str) -> String {
    let lower = title.to_lowercase();
    for article in ["the ", "an ", "a "] {
        if let Some(rest) = lower.strip_prefix(article) {
            return rest.trim_start().to_string();
        }
    }
    lower
}

/// Episode position parsed from a TV file, used to group episodes into series.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EpisodeInfo {
    pub series_title: String,
    pub series_year: Option<i32>,
    pub season: i32,
    pub episode: i32,
    /// Directory holding the show (used to locate series artwork).
    pub show_dir: Option<PathBuf>,
}

/// Read up to `max` ASCII digits starting at `start`, returning `(end, value)`.
/// `end` is the index just past the last digit consumed.
fn read_digits(chars: &[char], start: usize, max: usize) -> Option<(usize, i32)> {
    let mut end = start;
    let mut value: i32 = 0;
    while end < chars.len() && end - start < max && chars[end].is_ascii_digit() {
        value = value * 10 + (chars[end] as u32 - '0' as u32) as i32;
        end += 1;
    }
    if end == start {
        None
    } else {
        Some((end, value))
    }
}

/// Skip a single separator character (`.`, `_`, `-`, space) if present.
fn skip_separator(chars: &[char], index: usize) -> usize {
    if index < chars.len() && matches!(chars[index], '.' | '_' | '-' | ' ') {
        index + 1
    } else {
        index
    }
}

/// Locate a season/episode marker in a file stem, supporting `S01E02`,
/// `S01.E02`, `1x02` and similar spellings.
///
/// Returns `(start, end, season, episode)` as character indices, where the
/// match spans `chars[start..end]`. The marker must not be glued to a preceding
/// alphanumeric character, which avoids mistaking resolutions such as
/// `1280x720` for episodes.
fn find_season_episode(chars: &[char]) -> Option<(usize, usize, i32, i32)> {
    let n = chars.len();
    for i in 0..n {
        if i > 0 && chars[i - 1].is_ascii_alphanumeric() {
            continue;
        }
        // S##E##
        if chars[i] == 's' || chars[i] == 'S' {
            if let Some((after_season, season)) = read_digits(chars, i + 1, 2) {
                let e_index = skip_separator(chars, after_season);
                if e_index < n && (chars[e_index] == 'e' || chars[e_index] == 'E') {
                    let digits_at = skip_separator(chars, e_index + 1);
                    if let Some((end, episode)) = read_digits(chars, digits_at, 3) {
                        return Some((i, end, season, episode));
                    }
                }
            }
        }
        // ##x##
        if chars[i].is_ascii_digit() {
            if let Some((after_season, season)) = read_digits(chars, i, 2) {
                let x_index = skip_separator(chars, after_season);
                if x_index < n && (chars[x_index] == 'x' || chars[x_index] == 'X') {
                    let digits_at = skip_separator(chars, x_index + 1);
                    if let Some((end, episode)) = read_digits(chars, digits_at, 3) {
                        return Some((i, end, season, episode));
                    }
                }
            }
        }
    }
    None
}

/// Parse a season number from a folder name (`Season 1`, `S01`, `Specials`).
fn parse_season_folder(name: &str) -> Option<i32> {
    let lower = name.trim().to_ascii_lowercase();
    if lower == "specials" || lower == "special" {
        return Some(0);
    }
    for prefix in ["season ", "season.", "season_", "s"] {
        let Some(rest) = lower.strip_prefix(prefix) else {
            continue;
        };
        let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
        if digits.is_empty() {
            continue;
        }
        // A bare `s` prefix must be followed by digits only, so show names that
        // merely begin with an "s" are not treated as season folders.
        if prefix == "s" && rest.chars().any(|c| !c.is_ascii_digit()) {
            continue;
        }
        if let Ok(season) = digits.parse::<i32>() {
            return Some(season);
        }
    }
    None
}

/// Resolve the show directory for an episode and any season number encoded in
/// the immediate parent folder.
fn show_directory(path: &Path) -> (Option<PathBuf>, Option<i32>) {
    let Some(parent) = path.parent() else {
        return (None, None);
    };
    if let Some(name) = parent.file_name().and_then(|n| n.to_str()) {
        if let Some(season) = parse_season_folder(name) {
            if let Some(show) = parent.parent() {
                return (Some(show.to_path_buf()), Some(season));
            }
        }
    }
    (Some(parent.to_path_buf()), None)
}

/// Trim trailing separators left over before a season/episode marker.
fn clean_episode_prefix(prefix: &str) -> String {
    prefix
        .trim()
        .trim_end_matches(['.', '_', '-', ' '])
        .trim()
        .to_string()
}

/// Derive a series title (and year) from the show directory name.
fn series_from_dir(dir: Option<&Path>) -> (String, Option<i32>) {
    match dir.and_then(|d| d.file_name()).and_then(|n| n.to_str()) {
        Some(name) => parse_title_and_year(name),
        None => ("Unknown Series".to_string(), None),
    }
}

/// Parse series/season/episode information from a TV episode file.
///
/// The show title comes from the file name (everything before the episode
/// marker) or, failing that, the containing folder. Season falls back to a
/// `Season N` parent folder, and the episode is `0` when nothing can be found.
pub fn parse_episode(path: &Path) -> EpisodeInfo {
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("Untitled");
    let chars: Vec<char> = stem.chars().collect();
    let (show_dir, folder_season) = show_directory(path);

    if let Some((start, _end, season, episode)) = find_season_episode(&chars) {
        let prefix: String = chars[..start].iter().collect();
        let cleaned = clean_episode_prefix(&prefix);
        let (series_title, series_year) = if cleaned.is_empty() {
            series_from_dir(show_dir.as_deref())
        } else {
            parse_title_and_year(&cleaned)
        };
        return EpisodeInfo {
            series_title,
            series_year,
            season,
            episode,
            show_dir,
        };
    }

    let (series_title, series_year) = series_from_dir(show_dir.as_deref());
    EpisodeInfo {
        series_title,
        series_year,
        season: folder_season.unwrap_or(0),
        episode: 0,
        show_dir,
    }
}

/// Look for artwork inside a directory (used for series, whose artwork lives in
/// the show folder rather than beside a specific file).
fn find_dir_artwork(dir: &Path, kind: &str) -> Option<String> {
    let names: &[&str] = match kind {
        "poster" => &[
            "poster.jpg",
            "poster.png",
            "folder.jpg",
            "folder.png",
            "cover.jpg",
            "cover.png",
        ],
        _ => &[
            "backdrop.jpg",
            "backdrop.png",
            "fanart.jpg",
            "fanart.png",
            "background.jpg",
            "background.png",
        ],
    };
    for name in names {
        let candidate = dir.join(name);
        if candidate.is_file() {
            return Some(candidate.to_string_lossy().into_owned());
        }
    }
    None
}

/// Build the database row for the series an episode belongs to.
fn series_row(library_id: i64, info: &EpisodeInfo) -> NewSeries {
    let (poster, backdrop) = match info.show_dir.as_deref() {
        Some(dir) => (
            find_dir_artwork(dir, "poster"),
            find_dir_artwork(dir, "backdrop"),
        ),
        None => (None, None),
    };
    NewSeries {
        library_id,
        title: info.series_title.clone(),
        sort_title: sort_title(&info.series_title),
        year: info.series_year,
        overview: None,
        poster_path: poster,
        backdrop_path: backdrop,
    }
}

/// Map a library kind to the media kind its files represent.
fn media_kind_for(library_kind: LibraryKind) -> MediaKind {
    match library_kind {
        LibraryKind::Movies => MediaKind::Movie,
        LibraryKind::Tv => MediaKind::Episode,
        _ => MediaKind::Other,
    }
}

fn is_hidden(entry: &walkdir::DirEntry) -> bool {
    entry
        .file_name()
        .to_str()
        .map(|name| name.starts_with('.'))
        .unwrap_or(false)
}

fn extension_of(path: &Path) -> Option<String> {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
}

/// Find sidecar subtitle files (`<stem>.srt`, `<stem>.<lang>.srt`, `.vtt`, ...).
fn find_sidecars(video: &Path) -> Vec<SidecarTrack> {
    let Some(parent) = video.parent() else {
        return Vec::new();
    };
    let Some(stem) = video.file_stem().and_then(|s| s.to_str()) else {
        return Vec::new();
    };

    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Ok(entries) = std::fs::read_dir(parent) {
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            let ext = extension_of(&path).unwrap_or_default();
            if ext != "srt" && ext != "vtt" {
                continue;
            }
            if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                if name.starts_with(stem) && name[stem.len()..].starts_with('.')
                    || path.file_stem().and_then(|s| s.to_str()) == Some(stem)
                {
                    candidates.push(path);
                }
            }
        }
    }
    candidates.sort();

    candidates
        .into_iter()
        .enumerate()
        .map(|(i, path)| {
            let ext = extension_of(&path).unwrap_or_default();
            let language = path
                .file_stem()
                .and_then(|s| s.to_str())
                .and_then(|s| s.strip_prefix(stem))
                .map(|s| s.trim_start_matches('.').to_string())
                .filter(|s| !s.is_empty());
            SidecarTrack {
                index: SIDECAR_INDEX_BASE + i as u32,
                path: path.to_string_lossy().into_owned(),
                codec: ext,
                language,
                title: None,
                forced: false,
                default: i == 0,
            }
        })
        .collect()
}

/// Look for a sidecar artwork file next to the video.
fn find_artwork(video: &Path, kind: &str) -> Option<String> {
    let parent = video.parent()?;
    let stem = video.file_stem().and_then(|s| s.to_str()).unwrap_or("");
    let names: Vec<String> = match kind {
        "poster" => vec![
            "poster.jpg".into(),
            "poster.png".into(),
            "folder.jpg".into(),
            "folder.png".into(),
            format!("{stem}-poster.jpg"),
            format!("{stem}-poster.png"),
        ],
        _ => vec![
            "backdrop.jpg".into(),
            "backdrop.png".into(),
            "fanart.jpg".into(),
            "fanart.png".into(),
            format!("{stem}-backdrop.jpg"),
            format!("{stem}-fanart.jpg"),
        ],
    };
    for name in names {
        let candidate = parent.join(&name);
        if candidate.is_file() {
            return Some(candidate.to_string_lossy().into_owned());
        }
    }
    None
}

/// Build a media row from probed streams and local sidecars, using episode metadata when
/// supplied.
fn build_item(
    library_id: i64,
    kind: MediaKind,
    path: &Path,
    probed: &Probed,
    episode: Option<&EpisodeInfo>,
) -> NewMediaItem {
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("Untitled");
    let (title, year, sort) = match episode {
        Some(info) => {
            // Prefer a clean "Show S01E02" label; fall back to the file name when
            // no episode number could be parsed.
            let title = if info.episode > 0 {
                format!(
                    "{} S{:02}E{:02}",
                    info.series_title, info.season, info.episode
                )
            } else {
                parse_title_and_year(stem).0
            };
            (title, info.series_year, sort_title(&info.series_title))
        }
        None => {
            let (title, year) = parse_title_and_year(stem);
            let sort = sort_title(&title);
            (title, year, sort)
        }
    };
    NewMediaItem {
        library_id,
        kind,
        title,
        sort_title: sort,
        year,
        overview: None,
        file_path: path.to_string_lossy().into_owned(),
        file_size: probed.file_size.unwrap_or(0),
        container: probed.container.clone(),
        duration_ms: probed.duration_ms,
        streams: probed.streams.clone(),
        sidecars: find_sidecars(path),
        poster_path: find_artwork(path, "poster"),
        backdrop_path: find_artwork(path, "backdrop"),
        series_id: None,
        season: episode.map(|e| e.season),
        episode: episode.map(|e| e.episode),
    }
}

/// Re-probe and upsert a single item.
///
/// Used after writing a new subtitle sidecar so the item's sidecar tracks are
/// refreshed without a full library scan.
pub async fn reindex_item(pool: &SqlitePool, id: i64) -> anyhow::Result<()> {
    let item = db::get_item(pool, id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("media item {id} not found"))?;
    let path = PathBuf::from(&item.file_path);
    let probed = probe::probe_file(&path).await?;
    let episode = if item.kind == MediaKind::Episode {
        Some(parse_episode(&path))
    } else {
        None
    };
    let mut new_item = build_item(item.library_id, item.kind, &path, &probed, episode.as_ref());
    if let Some(info) = episode.as_ref() {
        let (series_id, _) = db::upsert_series(pool, &series_row(item.library_id, info)).await?;
        new_item.series_id = Some(series_id);
    }
    db::upsert_media(pool, &new_item).await?;
    Ok(())
}

/// Walk a library directory, returning matching media files.
async fn collect_media_files(
    root: PathBuf,
    extensions: HashSet<String>,
) -> (Vec<PathBuf>, Vec<String>) {
    tokio::task::spawn_blocking(move || {
        let mut files = Vec::new();
        let mut errors = Vec::new();
        if !root.is_dir() {
            errors.push(format!("{} is not a directory", root.display()));
            return (files, errors);
        }
        let walker = WalkDir::new(&root)
            .into_iter()
            .filter_entry(|e| !is_hidden(e));
        for entry in walker {
            match entry {
                Ok(entry) => {
                    if entry.file_type().is_file() {
                        if let Some(ext) = extension_of(entry.path()) {
                            if extensions.contains(&ext) {
                                files.push(entry.path().to_path_buf());
                            }
                        }
                    }
                }
                Err(err) => errors.push(err.to_string()),
            }
        }
        files.sort();
        (files, errors)
    })
    .await
    .unwrap_or_else(|e| (Vec::new(), vec![format!("walk task panicked: {e}")]))
}

/// Index a library, accumulate scan results and errors, and prune missing files and empty
/// series.
async fn scan_library(
    pool: &SqlitePool,
    library: &LibraryConfig,
    extensions: &HashSet<String>,
    report: &mut ScanReport,
) {
    let library_id =
        match db::upsert_library(pool, &library.name, &library.path, library.kind).await {
            Ok(id) => id,
            Err(err) => {
                report
                    .errors
                    .push(format!("library {}: {err}", library.name));
                return;
            }
        };

    let (files, walk_errors) =
        collect_media_files(PathBuf::from(&library.path), extensions.clone()).await;
    report.errors.extend(
        walk_errors
            .into_iter()
            .map(|e| format!("library {}: {e}", library.name)),
    );
    report.scanned += files.len();

    let kind = media_kind_for(library.kind);
    let probes = stream::iter(files.into_iter().map(|path| async move {
        let probed = probe::probe_file(&path).await;
        (path, probed)
    }))
    .buffer_unordered(PROBE_CONCURRENCY)
    .collect::<Vec<_>>()
    .await;

    let mut seen: HashSet<String> = HashSet::with_capacity(probes.len());
    for (path, probed) in probes {
        let path_str = path.to_string_lossy().into_owned();
        seen.insert(path_str.clone());
        match probed {
            Ok(probed) => {
                let episode = if kind == MediaKind::Episode {
                    Some(parse_episode(&path))
                } else {
                    None
                };
                let mut item = build_item(library_id, kind, &path, &probed, episode.as_ref());
                if let Some(info) = episode.as_ref() {
                    match db::upsert_series(pool, &series_row(library_id, info)).await {
                        Ok((series_id, _)) => item.series_id = Some(series_id),
                        Err(err) => report.errors.push(format!("{}: {err}", path.display())),
                    }
                }
                match db::upsert_media(pool, &item).await {
                    Ok((_, added)) => {
                        if added {
                            report.added += 1;
                        } else {
                            report.updated += 1;
                        }
                    }
                    Err(err) => report.errors.push(format!("{}: {err}", path.display())),
                }
            }
            Err(err) => report.errors.push(format!("{}: {err}", path.display())),
        }
    }

    // Remove database rows whose files are gone.
    match db::existing_paths(pool, library_id).await {
        Ok(existing) => {
            let removed: Vec<String> = existing.difference(&seen).cloned().collect();
            if !removed.is_empty() {
                match db::delete_paths(pool, &removed).await {
                    Ok(count) => report.removed += count,
                    Err(err) => report
                        .errors
                        .push(format!("library {}: {err}", library.name)),
                }
            }
        }
        Err(err) => report
            .errors
            .push(format!("library {}: {err}", library.name)),
    }

    // Drop series whose episodes were all removed.
    if kind == MediaKind::Episode {
        if let Err(err) = db::prune_series(pool, library_id).await {
            report
                .errors
                .push(format!("library {}: {err}", library.name));
        }
    }
}

/// Scan every configured library, returning an aggregate report.
pub async fn scan(pool: &SqlitePool, config: &Config) -> ScanReport {
    let extensions: HashSet<String> = config
        .extensions
        .iter()
        .map(|e| e.trim_start_matches('.').to_ascii_lowercase())
        .collect();

    let mut report = ScanReport {
        libraries: config.libraries.len(),
        ..Default::default()
    };

    for library in &config.libraries {
        scan_library(pool, library, &extensions, &mut report).await;
    }

    report
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_parenthesised_year() {
        let (title, year) = parse_title_and_year("Blade Runner 2049 (2017)");
        assert_eq!(title, "Blade Runner 2049");
        assert_eq!(year, Some(2017));
    }

    #[test]
    fn parses_trailing_year() {
        let (title, year) = parse_title_and_year("Sample.Movie.2020");
        assert_eq!(title, "Sample Movie");
        assert_eq!(year, Some(2020));

        let (title, year) = parse_title_and_year("Another_One.2021");
        assert_eq!(title, "Another One");
        assert_eq!(year, Some(2021));
    }

    #[test]
    fn no_year_when_absent() {
        let (title, year) = parse_title_and_year("Some_Show_S01E02");
        assert_eq!(title, "Some Show S01E02");
        assert_eq!(year, None);
    }

    #[test]
    fn year_like_word_is_not_stripped_mid_title() {
        let (title, year) = parse_title_and_year("2001 A Space Odyssey");
        // "2001" is the first token, not the trailing token, so it is preserved.
        assert_eq!(title, "2001 A Space Odyssey");
        assert_eq!(year, None);
    }

    #[test]
    fn parenthesised_year_can_be_mid_title() {
        let (title, year) = parse_title_and_year("The Matrix (1999) Remastered");
        assert_eq!(title, "The Matrix Remastered");
        assert_eq!(year, Some(1999));
    }

    #[test]
    fn strips_leading_articles_for_sort() {
        assert_eq!(sort_title("The Matrix"), "matrix");
        assert_eq!(sort_title("A Beautiful Mind"), "beautiful mind");
        assert_eq!(sort_title("An Education"), "education");
        assert_eq!(sort_title("Blade Runner"), "blade runner");
        // Only a leading article is removed.
        assert_eq!(sort_title("Theatre of Blood"), "theatre of blood");
    }

    /// Parse a path string into episode metadata for scanner tests.
    fn episode(path: &str) -> EpisodeInfo {
        parse_episode(Path::new(path))
    }

    /// Verify dotted file names supply the show title, season, and episode number.
    #[test]
    fn parses_dotted_season_episode() {
        let info = episode("/tv/The.Office.S03E05.720p.mkv");
        assert_eq!(info.series_title, "The Office");
        assert_eq!(info.series_year, None);
        assert_eq!(info.season, 3);
        assert_eq!(info.episode, 5);
    }

    /// Verify a release year before the episode marker is retained as series metadata.
    #[test]
    fn parses_compact_season_episode_with_year() {
        let info = episode("/tv/Breaking.Bad.2008.S01E01.mkv");
        assert_eq!(info.series_title, "Breaking Bad");
        assert_eq!(info.series_year, Some(2008));
        assert_eq!(info.season, 1);
        assert_eq!(info.episode, 1);
    }

    /// Verify an NxNN marker supplies episode numbers and falls back to the show folder title.
    #[test]
    fn parses_x_naming() {
        let info = episode("/tv/Firefly/1x02.mkv");
        assert_eq!(info.season, 1);
        assert_eq!(info.episode, 2);
        // No title before the marker, so the folder name is used.
        assert_eq!(info.series_title, "Firefly");
    }

    /// Verify a video resolution is ignored as an episode marker.
    #[test]
    fn resolution_is_not_mistaken_for_episode() {
        let info = episode("/tv/Some.Show.1280x720.mkv");
        // No episode marker; falls back to the folder name and episode 0.
        assert_eq!(info.series_title, "tv");
        assert_eq!(info.episode, 0);
    }

    /// Verify a season folder supplies the season when the file has no episode marker.
    #[test]
    fn season_folder_supplies_season() {
        let info = episode("/tv/The Wire/Season 2/episode title.mkv");
        assert_eq!(info.series_title, "The Wire");
        assert_eq!(info.season, 2);
        assert_eq!(info.episode, 0);
    }

    /// Verify supported season and specials folders while rejecting unrelated names.
    #[test]
    fn season_folder_parsing() {
        assert_eq!(parse_season_folder("Season 1"), Some(1));
        assert_eq!(parse_season_folder("season 01"), Some(1));
        assert_eq!(parse_season_folder("S03"), Some(3));
        assert_eq!(parse_season_folder("Specials"), Some(0));
        assert_eq!(parse_season_folder("Severance"), None);
        assert_eq!(parse_season_folder("Movies"), None);
    }

    /// Verify episode marker spellings return the expected character spans and numbers.
    #[test]
    fn season_episode_separator_variants() {
        assert_eq!(
            find_season_episode(&"S01.E02".chars().collect::<Vec<_>>()),
            Some((0, 7, 1, 2))
        );
        assert_eq!(
            find_season_episode(&"S1E2".chars().collect::<Vec<_>>()),
            Some((0, 4, 1, 2))
        );
        assert_eq!(
            find_season_episode(&"prefix 12x03".chars().collect::<Vec<_>>()),
            Some((7, 12, 12, 3))
        );
    }
}
