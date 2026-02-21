use anyhow::{Context, Result};
use sqlx::{Row, SqlitePool};

#[derive(Debug, Clone)]
pub struct Account {
    id: i64,
    label: String,
    imap_host: String,
    imap_port: i64,
    username: String,
    password: String,
    chat_id: i64,
    created_at: String,
}

impl Account {
    pub fn id(&self) -> i64 {
        self.id
    }

    pub fn label(&self) -> &str {
        &self.label
    }

    pub fn imap_host(&self) -> &str {
        &self.imap_host
    }

    pub fn imap_port(&self) -> u16 {
        self.imap_port as u16
    }

    pub fn username(&self) -> &str {
        &self.username
    }

    pub fn password(&self) -> &str {
        &self.password
    }

    pub fn chat_id(&self) -> i64 {
        self.chat_id
    }

    pub fn created_at(&self) -> &str {
        &self.created_at
    }
}

pub struct NewAccount<'a> {
    pub label: &'a str,
    pub imap_host: &'a str,
    pub imap_port: u16,
    pub username: &'a str,
    pub password: &'a str,
    pub chat_id: i64,
}

pub async fn init_db(path: &str) -> Result<SqlitePool> {
    let url = format!("sqlite:{path}?mode=rwc");
    let pool = SqlitePool::connect(&url)
        .await
        .with_context(|| format!("Failed to open SQLite database at {path}"))?;

    sqlx::migrate!("./migrations")
        .run(&pool)
        .await
        .context("Failed to run database migrations")?;

    Ok(pool)
}

pub async fn add_account(pool: &SqlitePool, account: &NewAccount<'_>) -> Result<i64> {
    let row = sqlx::query(
        "INSERT INTO accounts (label, imap_host, imap_port, username, password, chat_id) \
         VALUES (?, ?, ?, ?, ?, ?) RETURNING id",
    )
    .bind(account.label)
    .bind(account.imap_host)
    .bind(account.imap_port)
    .bind(account.username)
    .bind(account.password)
    .bind(account.chat_id)
    .fetch_one(pool)
    .await
    .context("Failed to insert account")?;

    Ok(row.get("id"))
}

pub async fn list_accounts(pool: &SqlitePool) -> Result<Vec<Account>> {
    let rows = sqlx::query(
        "SELECT id, label, imap_host, imap_port, username, password, chat_id, created_at \
         FROM accounts ORDER BY id",
    )
    .fetch_all(pool)
    .await
    .context("Failed to list accounts")?;

    let accounts = rows
        .into_iter()
        .map(|row| Account {
            id: row.get("id"),
            label: row.get("label"),
            imap_host: row.get("imap_host"),
            imap_port: row.get("imap_port"),
            username: row.get("username"),
            password: row.get("password"),
            chat_id: row.get("chat_id"),
            created_at: row.get("created_at"),
        })
        .collect();

    Ok(accounts)
}

pub async fn remove_account(pool: &SqlitePool, id: i64) -> Result<bool> {
    let result = sqlx::query("DELETE FROM accounts WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await
        .context("Failed to remove account")?;

    Ok(result.rows_affected() > 0)
}

pub async fn is_uid_seen(pool: &SqlitePool, account_id: i64, uid: &str) -> Result<bool> {
    let row = sqlx::query("SELECT COUNT(*) as cnt FROM seen_uids WHERE account_id = ? AND uid = ?")
        .bind(account_id)
        .bind(uid)
        .fetch_one(pool)
        .await
        .context("Failed to check seen UID")?;

    let count: i64 = row.get("cnt");
    Ok(count > 0)
}

pub async fn mark_uid_seen(pool: &SqlitePool, account_id: i64, uid: &str) -> Result<()> {
    sqlx::query("INSERT OR IGNORE INTO seen_uids (account_id, uid) VALUES (?, ?)")
        .bind(account_id)
        .bind(uid)
        .execute(pool)
        .await
        .context("Failed to mark UID as seen")?;

    Ok(())
}

/// Mark all existing UIDs as seen on first connect (no spam on startup).
pub async fn mark_all_uids_seen(pool: &SqlitePool, account_id: i64, uids: &[String]) -> Result<()> {
    for uid in uids {
        mark_uid_seen(pool, account_id, uid).await?;
    }
    Ok(())
}
