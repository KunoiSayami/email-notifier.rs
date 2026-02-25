use anyhow::Result;
use clap::Args;
use sqlx::SqlitePool;

use email_notifier::db;

#[derive(Args)]
pub struct RemoveArgs {
    /// ID of the account to remove (from `account list`)
    pub id: i64,
}

pub async fn run(pool: &SqlitePool, args: &RemoveArgs) -> Result<()> {
    let removed = db::remove_account(pool, args.id).await?;

    if removed {
        println!("Account {id} removed.", id = args.id);
    } else {
        println!("No account with ID {} found.", args.id);
    }

    Ok(())
}
