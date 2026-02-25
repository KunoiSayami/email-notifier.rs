use anyhow::Result;
use sqlx::SqlitePool;

use email_notifier::db;

pub async fn run(pool: &SqlitePool) -> Result<()> {
    let accounts = db::list_accounts(pool).await?;

    if accounts.is_empty() {
        println!("No accounts configured. Use `account add` to add one.");
        return Ok(());
    }

    println!(
        "{:<4} {:<20} {:<30} {:<5} {:<30} {:<12}",
        "ID", "Label", "IMAP Host", "Port", "Username", "Subscribers"
    );
    println!("{}", "-".repeat(105));

    for acct in &accounts {
        let sub_count = db::get_subscriber_count(pool, acct.id()).await?;
        println!(
            "{:<4} {:<20} {:<30} {:<5} {:<30} {:<12}",
            acct.id(),
            acct.label(),
            acct.imap_host(),
            acct.imap_port(),
            acct.username(),
            sub_count,
        );
    }

    Ok(())
}
