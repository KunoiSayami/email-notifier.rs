-- Recreate seen_uids with correct foreign key to email_accounts (was accounts, which was dropped in 004).

CREATE TABLE seen_uids_new (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    account_id INTEGER NOT NULL REFERENCES email_accounts(id) ON DELETE CASCADE,
    uid        TEXT NOT NULL,
    seen_at    TEXT NOT NULL DEFAULT (datetime('now')),
    UNIQUE(account_id, uid)
);

INSERT INTO seen_uids_new (id, account_id, uid, seen_at)
SELECT id, account_id, uid, seen_at FROM seen_uids;

DROP TABLE seen_uids;

ALTER TABLE seen_uids_new RENAME TO seen_uids;
