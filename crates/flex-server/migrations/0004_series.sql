-- TV series grouping: shows, seasons and the episode columns that link rows in
-- `media_items` back to their series.
--
-- The `series_id` / `season` / `episode` columns on `media_items` are added by
-- `db::ensure_series_columns` (see src/db.rs) rather than here, because
-- `ALTER TABLE ... ADD COLUMN` is not idempotent and this migration script is
-- re-applied on every startup.

CREATE TABLE IF NOT EXISTS series (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    library_id    INTEGER NOT NULL REFERENCES libraries (id) ON DELETE CASCADE,
    title         TEXT    NOT NULL,
    sort_title    TEXT    NOT NULL,
    year          INTEGER,
    overview      TEXT,
    poster_path   TEXT,
    backdrop_path TEXT,
    added_at      INTEGER NOT NULL,
    updated_at    INTEGER NOT NULL,
    -- Series are grouped per library by their normalised sort title.
    UNIQUE (library_id, sort_title)
);

CREATE INDEX IF NOT EXISTS idx_series_library    ON series (library_id);
CREATE INDEX IF NOT EXISTS idx_series_sort_title ON series (sort_title);
