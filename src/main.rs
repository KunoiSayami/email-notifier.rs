mod bot_commands;
mod cli;
mod config;
mod db;
mod email_formatter;
mod imap_monitor;
mod provider;
mod telegram;

use anyhow::{Context, Result};
use clap::Parser;
use cli::{AccountAction, Cli, Command};
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    // Load config to get the db path (needed for all subcommands).
    let config = config::load_config(std::path::Path::new(&cli.config))
        .with_context(|| format!("Failed to load config from '{}'", cli.config))?;

    // Initialize tracing
    let filter =
        EnvFilter::try_new(&config.log().level()).unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt().with_env_filter(filter).init();

    let pool = db::init_db(&config.database.path).await?;

    match cli.command {
        None | Some(Command::Run) => run_daemon(cli.config, config, pool).await?,
        Some(Command::Account { action }) => match action {
            AccountAction::Add(args) => cli::account_add::run(&pool, &args).await?,
            AccountAction::List => cli::account_list::run(&pool).await?,
            AccountAction::Remove(args) => cli::account_remove::run(&pool, &args).await?,
        },
    }

    Ok(())
}

async fn run_daemon(
    config_path: String,
    initial_config: config::Config,
    pool: sqlx::SqlitePool,
) -> Result<()> {
    tracing::info!("Starting email-notifier daemon…");

    // Watch config file for changes (affects bot token / global settings).
    let mut config_rx =
        config::watch_config(std::path::PathBuf::from(&config_path)).context("Config watcher")?;

    let mut current_config = initial_config;

    loop {
        let bot = telegram::create_bot(&current_config.telegram.bot_token);
        let admin_chat_id = current_config.telegram.admin_chat_id;

        // Spawn a monitor task per account.
        let accounts = db::list_accounts(&pool).await?;

        if accounts.is_empty() {
            tracing::warn!("No accounts configured. Add one with `email-notifier account add …`.");
        }

        let mut handles = Vec::new();
        for account in accounts {
            let pool_clone = pool.clone();
            let bot_clone = bot.clone();
            let handle = tokio::spawn(async move {
                imap_monitor::monitor_account(account, pool_clone, bot_clone).await;
            });
            handles.push(handle);
        }

        // Spawn the Telegram command handler (listens for /start etc.).
        let cmd_handle = tokio::spawn(bot_commands::run_command_handler(
            bot.clone(),
            pool.clone(),
            admin_chat_id,
        ));
        handles.push(cmd_handle);

        // Wait for a config change. When it arrives, cancel all tasks and restart with new config.
        config_rx.changed().await.ok();
        current_config = config_rx.borrow().clone();
        tracing::info!("Config changed, restarting monitors…");

        for handle in handles {
            handle.abort();
        }
    }
}
