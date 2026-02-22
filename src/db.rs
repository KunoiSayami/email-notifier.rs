use anyhow::{Context, Result};
use sqlx::SqlitePool;

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct EmailAccount {
    id: i64,
    label: String,
    imap_host: String,
    imap_port: i64,
    username: String,
    password: String,
    auth_method: String,
    refresh_token: Option<String>,
    access_token: Option<String>,
    #[allow(unused)]
    created_at: String,
}

impl EmailAccount {
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

    pub fn auth_method(&self) -> &str {
        &self.auth_method
    }

    pub fn refresh_token(&self) -> Option<&str> {
        self.refresh_token.as_deref()
    }

    pub fn is_oauth(&self) -> bool {
        self.auth_method == "xoauth2"
    }
}

pub struct NewAccount<'a> {
    pub label: &'a str,
    pub imap_host: &'a str,
    pub imap_port: u16,
    pub username: &'a str,
    pub password: &'a str,
    pub auth_method: &'a str,
    pub refresh_token: Option<&'a str>,
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

/// Upserts an email account and subscribes the given chat_id.
/// Returns the email_account id.
pub async fn add_account(pool: &SqlitePool, account: &NewAccount<'_>, chat_id: i64) -> Result<i64> {
    let account_id = sqlx::query_scalar::<_, i64>(
        "INSERT INTO email_accounts (label, imap_host, imap_port, username, password, auth_method, refresh_token) \
         VALUES (?, ?, ?, ?, ?, ?, ?) \
         ON CONFLICT(imap_host, imap_port, username) DO UPDATE SET \
           label = excluded.label, auth_method = excluded.auth_method, \
           refresh_token = excluded.refresh_token \
         RETURNING id",
    )
    .bind(account.label)
    .bind(account.imap_host)
    .bind(account.imap_port)
    .bind(account.username)
    .bind(account.password)
    .bind(account.auth_method)
    .bind(account.refresh_token)
    .fetch_one(pool)
    .await
    .context("Failed to upsert email account")?;

    sqlx::query("INSERT OR IGNORE INTO account_subscriptions (account_id, chat_id) VALUES (?, ?)")
        .bind(account_id)
        .bind(chat_id)
        .execute(pool)
        .await
        .context("Failed to add subscription")?;

    Ok(account_id)
}

/// List all email accounts (for daemon monitoring).
pub async fn list_accounts(pool: &SqlitePool) -> Result<Vec<EmailAccount>> {
    sqlx::query_as::<_, EmailAccount>(
        "SELECT id, label, imap_host, imap_port, username, password, \
                auth_method, refresh_token, access_token, created_at \
         FROM email_accounts ORDER BY id",
    )
    .fetch_all(pool)
    .await
    .context("Failed to list accounts")
}

/// List accounts that a specific chat_id is subscribed to.
pub async fn list_accounts_by_chat_id(
    pool: &SqlitePool,
    chat_id: i64,
) -> Result<Vec<EmailAccount>> {
    sqlx::query_as::<_, EmailAccount>(
        "SELECT e.id, e.label, e.imap_host, e.imap_port, e.username, e.password, \
                e.auth_method, e.refresh_token, e.access_token, e.created_at \
         FROM email_accounts e \
         JOIN account_subscriptions s ON s.account_id = e.id \
         WHERE s.chat_id = ? \
         ORDER BY e.id",
    )
    .bind(chat_id)
    .fetch_all(pool)
    .await
    .context("Failed to list accounts by chat_id")
}

/// Get all subscriber chat_ids for a given account.
pub async fn get_subscriber_chat_ids(pool: &SqlitePool, account_id: i64) -> Result<Vec<i64>> {
    sqlx::query_scalar::<_, i64>(
        "SELECT chat_id FROM account_subscriptions WHERE account_id = ? ORDER BY chat_id",
    )
    .bind(account_id)
    .fetch_all(pool)
    .await
    .context("Failed to get subscriber chat_ids")
}

/// Get the subscriber count for a given account.
pub async fn get_subscriber_count(pool: &SqlitePool, account_id: i64) -> Result<i64> {
    sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM account_subscriptions WHERE account_id = ?")
        .bind(account_id)
        .fetch_one(pool)
        .await
        .context("Failed to get subscriber count")
}

/// Remove an email account entirely (admin / CLI use).
pub async fn remove_account(pool: &SqlitePool, id: i64) -> Result<bool> {
    let result = sqlx::query("DELETE FROM email_accounts WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await
        .context("Failed to remove account")?;

    Ok(result.rows_affected() > 0)
}

/// Unsubscribe a chat_id from an account. If no subscribers remain, delete the account too.
pub async fn remove_account_by_id_and_chat_id(
    pool: &SqlitePool,
    id: i64,
    chat_id: i64,
) -> Result<bool> {
    let result =
        sqlx::query("DELETE FROM account_subscriptions WHERE account_id = ? AND chat_id = ?")
            .bind(id)
            .bind(chat_id)
            .execute(pool)
            .await
            .context("Failed to remove subscription")?;

    if result.rows_affected() == 0 {
        return Ok(false);
    }

    // If no subscribers remain, delete the email account (cascades to seen_uids).
    let remaining = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM account_subscriptions WHERE account_id = ?",
    )
    .bind(id)
    .fetch_one(pool)
    .await
    .context("Failed to count remaining subscribers")?;

    if remaining == 0 {
        sqlx::query("DELETE FROM email_accounts WHERE id = ?")
            .bind(id)
            .execute(pool)
            .await
            .context("Failed to remove orphaned account")?;
    }

    Ok(true)
}

/// Cache a refreshed OAuth access token in the database.
pub async fn update_access_token(
    pool: &SqlitePool,
    account_id: i64,
    access_token: &str,
) -> Result<()> {
    sqlx::query("UPDATE email_accounts SET access_token = ? WHERE id = ?")
        .bind(access_token)
        .bind(account_id)
        .execute(pool)
        .await
        .context("Failed to update access token")?;
    Ok(())
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
