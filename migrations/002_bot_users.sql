CREATE TABLE IF NOT EXISTS bot_users (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    chat_id    INTEGER NOT NULL UNIQUE,
    username   TEXT,
    first_name TEXT,
    last_name  TEXT,
    started_at TEXT NOT NULL DEFAULT (datetime('now'))
);
