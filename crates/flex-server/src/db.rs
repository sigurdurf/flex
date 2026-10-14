//! SQLite persistence layer.

use std::collections::HashSet;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use flex_protocol::{ItemQuery, Library, MediaItem, Page};
use sqlx::sqlite::{SqliteConnectOptions, SqlitePool, SqlitePoolOptions};
use sqlx::{QueryBuilder, Row, Sqlite};

use crate::model::{library_kind_str, row_to_media_item, NewMediaItem, SidecarTrack};

/// Current Unix time in seconds.
pub fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Open (creating if needed) the SQLite database and run migrations.
pub async fn init_pool(database: &str) -> anyhow::Result<SqlitePool> {
    if let Some(parent) = Path::new(database).parent() {
        if !parent.as_os_str().is_empty() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| anyhow::anyhow!("failed to create {}: {e}", parent.display()))?;
        }
    }

    let options = SqliteConnectOptions::new()
        .filename(database)
        .create_if_missing(true)
        .foreign_keys(true);

    let pool = SqlitePoolOptions::new()
        .max_connections(5)
        .connect_with(options)
        .await
        .map_err(|e| anyhow::anyhow!("failed to open database {database}: {e}"))?;

    sqlx::raw_sql(include_str!("../migrations/0001_init.sql"))
        .execute(&pool)
        .await
        .map_err(|e| anyhow::anyhow!("database migration failed: {e}"))?;

    Ok(pool)
}

/// Insert a library if its path is new, otherwise refresh name/kind.
///
/// Returns the library id.
pub async fn upsert_library(
    pool: &SqlitePool,
    name: &str,
    path: &str,
    kind: flex_protocol::LibraryKind,
) -> Result<i64, sqlx::Error> {
    let kind = library_kind_str(kind);
    if let Some(row) = sqlx::query("SELECT id FROM libraries WHERE path = ?")
        .bind(path)
        .fetch_optional(pool)
        .await?
    {
        let id: i64 = row.get("id");
        sqlx::query("UPDATE libraries SET name = ?, kind = ? WHERE id = ?")
            .bind(name)
            .bind(kind)
            .bind(id)
            .execute(pool)
            .await?;
        return Ok(id);
    }

    let result =
        sqlx::query("INSERT INTO libraries (name, path, kind, created_at) VALUES (?, ?, ?, ?)")
            .bind(name)
            .bind(path)
            .bind(kind)
            .bind(now_secs())
            .execute(pool)
            .await?;
    Ok(result.last_insert_rowid())
}

/// List all libraries with their current item counts.
pub async fn list_libraries(pool: &SqlitePool) -> Result<Vec<Library>, sqlx::Error> {
    let rows = sqlx::query(
        "SELECT l.id, l.name, l.path, l.kind, l.created_at, \
         (SELECT COUNT(*) FROM media_items m WHERE m.library_id = l.id) AS item_count \
         FROM libraries l ORDER BY l.name COLLATE NOCASE",
    )
    .fetch_all(pool)
    .await?;

    rows.iter()
        .map(|row| {
            let kind: String = row.try_get("kind")?;
            Ok(Library {
                id: row.try_get("id")?,
                name: row.try_get("name")?,
                path: row.try_get("path")?,
                kind: crate::model::library_kind_from_str(&kind),
                created_at: row.try_get("created_at")?,
                item_count: row.try_get("item_count")?,
            })
        })
        .collect()
}

/// Every indexed file path belonging to a library.
pub async fn existing_paths(
    pool: &SqlitePool,
    library_id: i64,
) -> Result<HashSet<String>, sqlx::Error> {
    let rows = sqlx::query("SELECT file_path FROM media_items WHERE library_id = ?")
        .bind(library_id)
        .fetch_all(pool)
        .await?;
    rows.iter()
        .map(|row| row.try_get::<String, _>("file_path"))
        .collect()
}

/// Delete rows by file path, returning how many were removed.
pub async fn delete_paths(pool: &SqlitePool, paths: &[String]) -> Result<usize, sqlx::Error> {
    let mut removed = 0;
    for path in paths {
        let result = sqlx::query("DELETE FROM media_items WHERE file_path = ?")
            .bind(path)
            .execute(pool)
            .await?;
        removed += result.rows_affected() as usize;
    }
    Ok(removed)
}

/// Insert or update a media item keyed on `file_path`.
///
/// Returns `(id, added)` where `added` is `true` for a new row. `added_at` is
/// preserved on update while `updated_at` is bumped.
pub async fn upsert_media(
    pool: &SqlitePool,
    item: &NewMediaItem,
) -> Result<(i64, bool), sqlx::Error> {
    let now = now_secs();
    let video_json = item
        .streams
        .video
        .as_ref()
        .map(|v| serde_json::to_string(v).unwrap_or_default());
    let audio_json = serde_json::to_string(&item.streams.audio).unwrap_or_else(|_| "[]".into());
    let subs_json = serde_json::to_string(&item.streams.subtitles).unwrap_or_else(|_| "[]".into());
    let sidecars_json = serde_json::to_string(&item.sidecars).unwrap_or_else(|_| "[]".into());
    let kind = crate::model::media_kind_str(item.kind);

    let existing: Option<i64> =
        sqlx::query_scalar("SELECT id FROM media_items WHERE file_path = ?")
            .bind(&item.file_path)
            .fetch_optional(pool)
            .await?;

    if let Some(id) = existing {
        sqlx::query(
            "UPDATE media_items SET library_id = ?, kind = ?, title = ?, sort_title = ?, year = ?, \
             overview = ?, file_size = ?, container = ?, duration_ms = ?, video_json = ?, \
             audio_json = ?, subs_json = ?, sidecars_json = ?, poster_path = ?, backdrop_path = ?, \
             updated_at = ? WHERE id = ?",
        )
        .bind(item.library_id)
        .bind(kind)
        .bind(&item.title)
        .bind(&item.sort_title)
        .bind(item.year)
        .bind(&item.overview)
        .bind(item.file_size as i64)
        .bind(&item.container)
        .bind(item.duration_ms.map(|v| v as i64))
        .bind(&video_json)
        .bind(&audio_json)
        .bind(&subs_json)
        .bind(&sidecars_json)
        .bind(&item.poster_path)
        .bind(&item.backdrop_path)
        .bind(now)
        .bind(id)
        .execute(pool)
        .await?;
        Ok((id, false))
    } else {
        let result = sqlx::query(
            "INSERT INTO media_items (library_id, kind, title, sort_title, year, overview, \
             file_path, file_size, container, duration_ms, video_json, audio_json, subs_json, \
             sidecars_json, poster_path, backdrop_path, added_at, updated_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(item.library_id)
        .bind(kind)
        .bind(&item.title)
        .bind(&item.sort_title)
        .bind(item.year)
        .bind(&item.overview)
        .bind(&item.file_path)
        .bind(item.file_size as i64)
        .bind(&item.container)
        .bind(item.duration_ms.map(|v| v as i64))
        .bind(&video_json)
        .bind(&audio_json)
        .bind(&subs_json)
        .bind(&sidecars_json)
        .bind(&item.poster_path)
        .bind(&item.backdrop_path)
        .bind(now)
        .bind(now)
        .execute(pool)
        .await?;
        Ok((result.last_insert_rowid(), true))
    }
}

/// Fetch a single media item by id.
pub async fn get_item(pool: &SqlitePool, id: i64) -> Result<Option<MediaItem>, sqlx::Error> {
    let row = sqlx::query("SELECT * FROM media_items WHERE id = ?")
        .bind(id)
        .fetch_optional(pool)
        .await?;
    row.as_ref().map(row_to_media_item).transpose()
}

/// Fetch the on-disk artwork paths for an item.
pub async fn get_artwork_paths(
    pool: &SqlitePool,
    id: i64,
) -> Result<Option<(Option<String>, Option<String>)>, sqlx::Error> {
    let row = sqlx::query("SELECT poster_path, backdrop_path FROM media_items WHERE id = ?")
        .bind(id)
        .fetch_optional(pool)
        .await?;
    Ok(row.map(|r| (r.get("poster_path"), r.get("backdrop_path"))))
}

/// Fetch the sidecar subtitle tracks for an item.
pub async fn get_sidecars(pool: &SqlitePool, id: i64) -> Result<Vec<SidecarTrack>, sqlx::Error> {
    let raw: Option<String> =
        sqlx::query_scalar("SELECT sidecars_json FROM media_items WHERE id = ?")
            .bind(id)
            .fetch_optional(pool)
            .await?
            .flatten();
    Ok(raw
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default())
}

/// Fetch the embedded subtitle streams for an item.
pub async fn get_embedded_subtitles(
    pool: &SqlitePool,
    id: i64,
) -> Result<Vec<flex_protocol::SubtitleStream>, sqlx::Error> {
    let raw: Option<String> = sqlx::query_scalar("SELECT subs_json FROM media_items WHERE id = ?")
        .bind(id)
        .fetch_optional(pool)
        .await?
        .flatten();
    Ok(raw
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default())
}

fn apply_filters(qb: &mut QueryBuilder<'_, Sqlite>, query: &ItemQuery) {
    let mut first = true;
    let mut and = |qb: &mut QueryBuilder<'_, Sqlite>| {
        if first {
            qb.push(" WHERE ");
            first = false;
        } else {
            qb.push(" AND ");
        }
    };
    if let Some(library_id) = query.library_id {
        and(qb);
        qb.push("library_id = ").push_bind(library_id);
    }
    if let Some(kind) = query.kind {
        and(qb);
        qb.push("kind = ")
            .push_bind(crate::model::media_kind_str(kind));
    }
    if let Some(q) = query.q.as_ref().map(|q| q.trim()).filter(|q| !q.is_empty()) {
        and(qb);
        let pattern = format!("%{}%", q.to_ascii_lowercase());
        qb.push("LOWER(title) LIKE ").push_bind(pattern);
    }
}

fn order_clause(sort: Option<&str>) -> &'static str {
    match sort.unwrap_or("title") {
        "-title" => "sort_title DESC",
        "year" => "(year IS NULL) ASC, year ASC, sort_title ASC",
        "-year" => "(year IS NULL) ASC, year DESC, sort_title ASC",
        "added" => "added_at ASC, id ASC",
        "-added" => "added_at DESC, id DESC",
        _ => "sort_title ASC, id ASC",
    }
}

/// Query media items with filtering, sorting and pagination.
pub async fn list_items(
    pool: &SqlitePool,
    query: &ItemQuery,
) -> Result<Page<MediaItem>, sqlx::Error> {
    let limit = query.limit.unwrap_or(50).clamp(0, 500);
    let offset = query.offset.unwrap_or(0).max(0);

    let mut count_qb = QueryBuilder::<Sqlite>::new("SELECT COUNT(*) FROM media_items");
    apply_filters(&mut count_qb, query);
    let total: i64 = count_qb.build_query_scalar().fetch_one(pool).await?;

    let mut qb = QueryBuilder::<Sqlite>::new("SELECT * FROM media_items");
    apply_filters(&mut qb, query);
    qb.push(" ORDER BY ")
        .push(order_clause(query.sort.as_deref()));
    qb.push(" LIMIT ")
        .push_bind(limit)
        .push(" OFFSET ")
        .push_bind(offset);

    let rows = qb.build().fetch_all(pool).await?;
    let items = rows
        .iter()
        .map(row_to_media_item)
        .collect::<Result<Vec<_>, _>>()?;

    Ok(Page {
        items,
        total,
        limit,
        offset,
    })
}
