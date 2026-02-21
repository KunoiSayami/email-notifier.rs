use anyhow::Result;
use clap::Args;
use sqlx::SqlitePool;

use crate::db::{self, NewAccount};

#[derive(Args)]
pub struct AddArgs {
    /// Human-readable label for this account
    #[arg(long)]
    pub label: String,

    /// IMAP server hostname
    #[arg(long)]
    pub host: String,

    /// IMAP server port
    #[arg(long, default_value_t = 993)]
    pub port: u16,

    /// Email address / IMAP login username
    #[arg(long)]
    pub username: String,

    /// IMAP password or app password
    #[arg(long)]
    pub password: String,

    /// Telegram chat ID to send notifications to
    #[arg(long)]
    pub chat_id: i64,
}

pub async fn run(pool: &SqlitePool, args: &AddArgs) -> Result<()> {
    let id = db::add_account(
        pool,
        &NewAccount {
            label: &args.label,
            imap_host: &args.host,
            imap_port: args.port,
            username: &args.username,
            password: &args.password,
            chat_id: args.chat_id,
        },
    )
    .await?;

    println!("Account added with ID {id}.");
    println!("  Label:    {}", args.label);
    println!("  Host:     {}:{}", args.host, args.port);
    println!("  Username: {}", args.username);
    println!("  Chat ID:  {}", args.chat_id);

    Ok(())
}
