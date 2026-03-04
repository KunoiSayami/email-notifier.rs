CREATE TABLE IF NOT EXISTS email_bodies (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    header_html TEXT NOT NULL,
    full_body TEXT NOT NULL,
    attachments_html TEXT NOT NULL DEFAULT '',
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);
