-- Deduplicate email accounts: split accounts into email_accounts + account_subscriptions.

-- 1. Create new tables
CREATE TABLE email_accounts (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    label      TEXT NOT NULL,
    imap_host  TEXT NOT NULL,
    imap_port  INTEGER NOT NULL DEFAULT 993,
    username   TEXT NOT NULL,
    password   TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    UNIQUE(imap_host, imap_port, username)
);

CREATE TABLE account_subscriptions (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    account_id INTEGER NOT NULL REFERENCES email_accounts(id) ON DELETE CASCADE,
    chat_id    INTEGER NOT NULL,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    UNIQUE(account_id, chat_id)
);

-- 2. Migrate distinct credentials into email_accounts
INSERT INTO email_accounts (label, imap_host, imap_port, username, password, created_at)
SELECT label, imap_host, imap_port, username, password, MIN(created_at)
FROM accounts
GROUP BY imap_host, imap_port, username;

-- 3. Create subscriptions from old accounts
INSERT INTO account_subscriptions (account_id, chat_id, created_at)
SELECT e.id, a.chat_id, a.created_at
FROM accounts a
JOIN email_accounts e
  ON a.imap_host = e.imap_host
 AND a.imap_port = e.imap_port
 AND a.username  = e.username;

-- 4. Remap seen_uids.account_id to new email_accounts.id
UPDATE seen_uids
SET account_id = (
    SELECT e.id
    FROM accounts a
    JOIN email_accounts e
      ON a.imap_host = e.imap_host
     AND a.imap_port = e.imap_port
     AND a.username  = e.username
    WHERE a.id = seen_uids.account_id
);

-- 5. Drop old table
DROP TABLE accounts;
