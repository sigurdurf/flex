-- Per-user watch state ("continue watching" / resume) support.

CREATE TABLE IF NOT EXISTS watch_state (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id     INTEGER NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    item_id     INTEGER NOT NULL REFERENCES media_items (id) ON DELETE CASCADE,
    position_ms INTEGER NOT NULL DEFAULT 0,
    duration_ms INTEGER,
    watched     INTEGER NOT NULL DEFAULT 0,
    updated_at  INTEGER NOT NULL,
    UNIQUE (user_id, item_id)
);

CREATE INDEX IF NOT EXISTS idx_watch_state_user_updated
    ON watch_state (user_id, updated_at DESC);
