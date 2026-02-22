use anyhow::{Context, Result};
use sqlx::SqlitePool;

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Account {
    id: i64,
    label: String,
    imap_host: String,
    imap_port: i64,
    username: String,
    password: String,
    chat_id: i64,
    #[allow(unused)]
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

    #[allow(unused)]
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
    let id = sqlx::query_scalar::<_, i64>(
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

    Ok(id)
}

pub async fn list_accounts(pool: &SqlitePool) -> Result<Vec<Account>> {
    sqlx::query_as::<_, Account>(
        "SELECT id, label, imap_host, imap_port, username, password, chat_id, created_at \
         FROM accounts ORDER BY id",
    )
    .fetch_all(pool)
    .await
    .context("Failed to list accounts")
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
    let count = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM seen_uids WHERE account_id = ? AND uid = ?",
    )
    .bind(account_id)
    .bind(uid)
    .fetch_one(pool)
    .await
    .context("Failed to check seen UID")?;

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

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct BotUser {
    id: i64,
    chat_id: i64,
    username: Option<String>,
    first_name: Option<String>,
    last_name: Option<String>,
    #[allow(unused)]
    started_at: String,
    privilege: i64,
}

impl BotUser {
    pub fn id(&self) -> i64 {
        self.id
    }

    pub fn chat_id(&self) -> i64 {
        self.chat_id
    }

    pub fn username(&self) -> Option<&str> {
        self.username.as_deref()
    }

    pub fn first_name(&self) -> Option<&str> {
        self.first_name.as_deref()
    }

    pub fn last_name(&self) -> Option<&str> {
        self.last_name.as_deref()
    }

    #[allow(unused)]
    pub fn started_at(&self) -> &str {
        &self.started_at
    }

    pub fn privilege(&self) -> i64 {
        self.privilege
    }
}

/// Returns `true` if this is a newly inserted user, `false` if they already existed.
pub async fn register_bot_user(
    pool: &SqlitePool,
    chat_id: i64,
    username: Option<&str>,
    first_name: Option<&str>,
    last_name: Option<&str>,
) -> Result<bool> {
    let result = sqlx::query(
        "INSERT OR IGNORE INTO bot_users (chat_id, username, first_name, last_name) \
         VALUES (?, ?, ?, ?)",
    )
    .bind(chat_id)
    .bind(username)
    .bind(first_name)
    .bind(last_name)
    .execute(pool)
    .await
    .context("Failed to register bot user")?;

    Ok(result.rows_affected() > 0)
}

/// Mark all existing UIDs as seen on first connect (no spam on startup).
pub async fn mark_all_uids_seen(pool: &SqlitePool, account_id: i64, uids: &[String]) -> Result<()> {
    for uid in uids {
        mark_uid_seen(pool, account_id, uid).await?;
    }
    Ok(())
}

/// Check if a user has privilege >= 1 in bot_users.
pub async fn is_user_allowed(pool: &SqlitePool, chat_id: i64) -> Result<bool> {
    let privilege = sqlx::query_scalar::<_, i64>(
        "SELECT COALESCE((SELECT privilege FROM bot_users WHERE chat_id = ?), 0)",
    )
    .bind(chat_id)
    .fetch_one(pool)
    .await
    .context("Failed to check user privilege")?;

    Ok(privilege >= 1)
}

/// Grant privilege = 1 to a user. Returns false if the user hasn't /start-ed yet.
pub async fn allow_user(pool: &SqlitePool, chat_id: i64) -> Result<bool> {
    let result = sqlx::query("UPDATE bot_users SET privilege = 1 WHERE chat_id = ?")
        .bind(chat_id)
        .execute(pool)
        .await
        .context("Failed to allow user")?;

    Ok(result.rows_affected() > 0)
}

/// List accounts belonging to a specific chat_id.
pub async fn list_accounts_by_chat_id(pool: &SqlitePool, chat_id: i64) -> Result<Vec<Account>> {
    sqlx::query_as::<_, Account>(
        "SELECT id, label, imap_host, imap_port, username, password, chat_id, created_at \
         FROM accounts WHERE chat_id = ? ORDER BY id",
    )
    .bind(chat_id)
    .fetch_all(pool)
    .await
    .context("Failed to list accounts by chat_id")
}

/// Remove an account only if it belongs to the given chat_id.
pub async fn remove_account_by_id_and_chat_id(
    pool: &SqlitePool,
    id: i64,
    chat_id: i64,
) -> Result<bool> {
    let result = sqlx::query("DELETE FROM accounts WHERE id = ? AND chat_id = ?")
        .bind(id)
        .bind(chat_id)
        .execute(pool)
        .await
        .context("Failed to remove account")?;

    Ok(result.rows_affected() > 0)
}
