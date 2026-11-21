//! Confine library file access to the library root that owns the item.
//!
//! [`confined_file`] is the check. HTTP handlers load the root with
//! [`library_file`]; the scanner already knows the root it is walking.

use std::path::{Path, PathBuf};

use sqlx::SqlitePool;

use crate::db;
use crate::error::AppError;

fn unavailable() -> AppError {
    AppError::not_found("file is unavailable")
}

/// Load the library root for `library_id` and resolve `candidate` inside it.
pub async fn library_file(
    pool: &SqlitePool,
    library_id: i64,
    candidate: &Path,
) -> Result<PathBuf, AppError> {
    let root = db::library_path(pool, library_id)
        .await?
        .ok_or_else(unavailable)?;
    confined_file(Path::new(&root), candidate)
}

/// Canonical path of a regular file inside `library_root`.
///
/// Both paths are canonicalized, so symlinks are followed before the check.
/// The candidate must be the root or strictly inside it on a component
/// boundary (`/media` does not accept `/media-evil`), and it must be a
/// regular file. Failure is a 404 that does not include a filesystem path.
pub fn confined_file(library_root: &Path, candidate: &Path) -> Result<PathBuf, AppError> {
    let path = canonicalize_within(library_root, candidate)?;
    match std::fs::symlink_metadata(&path) {
        Ok(metadata) if metadata.file_type().is_file() => Ok(path),
        _ => Err(unavailable()),
    }
}

/// Canonicalize `candidate` and require it to sit inside `library_root`.
///
/// Does not require a regular file, so a sidecar's existing parent directory
/// can use the same boundary rule. `Path::starts_with` is only applied after
/// canonicalization: it does not resolve `..`.
pub fn canonicalize_within(library_root: &Path, candidate: &Path) -> Result<PathBuf, AppError> {
    let root = std::fs::canonicalize(library_root).map_err(|_| unavailable())?;
    let path = std::fs::canonicalize(candidate).map_err(|_| unavailable())?;
    if path.starts_with(&root) {
        Ok(path)
    } else {
        Err(unavailable())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_regular_file_inside_library_and_rejects_outside() {
        let root = tempfile::tempdir().unwrap();
        let inside = root.path().join("movie.mkv");
        std::fs::write(&inside, b"video").unwrap();

        let outside_dir = tempfile::tempdir().unwrap();
        let outside = outside_dir.path().join("secret.txt");
        std::fs::write(&outside, b"nope").unwrap();

        let resolved = confined_file(root.path(), &inside).unwrap();
        assert_eq!(resolved, std::fs::canonicalize(&inside).unwrap());

        let err = confined_file(root.path(), &outside).unwrap_err();
        assert!(matches!(err, AppError::NotFound(_)));
        let message = err.to_string();
        assert!(!message.contains(outside.to_str().unwrap()));
        assert!(!message.contains(outside_dir.path().to_str().unwrap()));

        // The library root itself is a directory, not a file we can stream.
        assert!(confined_file(root.path(), root.path()).is_err());
    }

    #[test]
    fn rejects_sibling_directory_with_shared_prefix() {
        let parent = tempfile::tempdir().unwrap();
        let root = parent.path().join("media");
        let evil = parent.path().join("media-evil");
        std::fs::create_dir(&root).unwrap();
        std::fs::create_dir(&evil).unwrap();
        let file = evil.join("movie.mkv");
        std::fs::write(&file, b"x").unwrap();
        assert!(confined_file(&root, &file).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn symlink_inside_library_cannot_escape() {
        let root = tempfile::tempdir().unwrap();
        let outside_dir = tempfile::tempdir().unwrap();
        let secret = outside_dir.path().join("passwd");
        std::fs::write(&secret, b"secret").unwrap();
        let link = root.path().join("movie.mkv");
        std::os::unix::fs::symlink(&secret, &link).unwrap();
        let err = confined_file(root.path(), &link).unwrap_err();
        assert!(matches!(err, AppError::NotFound(_)));
        assert!(!err.to_string().contains("passwd"));

        let inside = root.path().join("inside.mkv");
        std::fs::write(&inside, b"ok").unwrap();
        let alias = root.path().join("alias.mkv");
        std::os::unix::fs::symlink(&inside, &alias).unwrap();
        assert!(confined_file(root.path(), &alias).is_ok());
    }
}
