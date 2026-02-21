use anyhow::Result;
use sqlx::SqlitePool;

use crate::db;

pub async fn run(pool: &SqlitePool) -> Result<()> {
    let accounts = db::list_accounts(pool).await?;

    if accounts.is_empty() {
        println!("No accounts configured. Use `account add` to add one.");
        return Ok(());
    }

    println!(
        "{:<4} {:<20} {:<30} {:<5} {:<30} {:<15}",
        "ID", "Label", "IMAP Host", "Port", "Username", "Chat ID"
    );
    println!("{}", "-".repeat(110));

    for acct in &accounts {
        println!(
            "{:<4} {:<20} {:<30} {:<5} {:<30} {:<15}",
            acct.id(),
            acct.label(),
            acct.imap_host(),
            acct.imap_port(),
            acct.username(),
            acct.chat_id(),
        );
    }

    Ok(())
}
