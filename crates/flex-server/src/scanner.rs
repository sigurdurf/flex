//! Library scanning: walking directories, probing files and persisting results.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use flex_protocol::{LibraryKind, MediaKind, ScanReport};
use futures::stream::{self, StreamExt};
use sqlx::SqlitePool;
use walkdir::WalkDir;

use crate::config::{Config, LibraryConfig};
use crate::model::{NewMediaItem, SidecarTrack, SIDECAR_INDEX_BASE};
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

fn build_item(library_id: i64, kind: MediaKind, path: &Path, probed: &Probed) -> NewMediaItem {
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("Untitled");
    let (title, year) = parse_title_and_year(stem);
    let sort = sort_title(&title);
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
    }
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
                let item = build_item(library_id, kind, &path, &probed);
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
}
