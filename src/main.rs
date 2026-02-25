mod cli;

use std::sync::Arc;

use anyhow::{Context, Result};
use clap::Parser;
use cli::{AccountAction, Cli, Command, IpcAction};
use email_notifier::{bot_commands, config, db, imap_monitor, ipc, telegram};
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;
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

    match cli.command {
        None | Some(Command::Run) => {
            let pool = db::init_db(&config.database.path).await?;
            run_daemon(cli.config, config, pool).await?;
        }
        Some(Command::Account { action }) => {
            let pool = db::init_db(&config.database.path).await?;
            match action {
                AccountAction::Add(args) => cli::account_add::run(&pool, &args).await?,
                AccountAction::List => cli::account_list::run(&pool).await?,
                AccountAction::Remove(args) => cli::account_remove::run(&pool, &args).await?,
            }
        }
        Some(Command::Ipc { action }) => {
            run_ipc_client(&config, &action).await?;
        }
    }

    Ok(())
}

async fn run_ipc_client(config: &config::Config, action: &IpcAction) -> Result<()> {
    cli::ipc_client::run(config.ipc().socket_name(), action).await
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
    let reload_notify = Arc::new(Notify::new());
    let shutdown_token = CancellationToken::new();

    // Create an initial bot instance for the IPC server (lives for the entire daemon lifetime).
    let ipc_bot = telegram::create_bot(&current_config.telegram.bot_token);

    // Spawn IPC server (lives for the entire daemon lifetime).
    let ipc_socket_name = current_config.ipc().socket_name().to_owned();
    let ipc_pool = pool.clone();
    let ipc_reload = reload_notify.clone();
    let ipc_cancel = shutdown_token.child_token();
    let ipc_handle = tokio::spawn(async move {
        if let Err(e) =
            ipc::run_ipc_server(ipc_socket_name, ipc_pool, ipc_bot, ipc_reload, ipc_cancel).await
        {
            tracing::error!("IPC server error: {e:#}");
        }
    });

    loop {
        let bot = telegram::create_bot(&current_config.telegram.bot_token);
        let admin_chat_id = current_config.telegram.admin_chat_id;

        // Spawn a monitor task per unique email account.
        let accounts = db::list_accounts(&pool).await?;

        if accounts.is_empty() {
            tracing::warn!("No accounts configured. Add one with `email-notifier account add …`.");
        }

        let oauth_config = current_config.oauth().clone();
        let mut handles = Vec::new();
        for account in accounts {
            let pool_clone = pool.clone();
            let bot_clone = bot.clone();
            let oauth_clone = oauth_config.clone();
            let cancel = shutdown_token.child_token();
            let handle = tokio::spawn(async move {
                imap_monitor::monitor_account(account, pool_clone, bot_clone, oauth_clone, cancel)
                    .await;
            });
            handles.push(handle);
        }

        // Spawn the Telegram command handler (listens for /start etc.).
        let cmd_handle = tokio::spawn(bot_commands::run_command_handler(
            bot.clone(),
            pool.clone(),
            admin_chat_id,
            oauth_config.clone(),
            shutdown_token.child_token(),
        ));
        handles.push(cmd_handle);

        // Wait for a config change, IPC reload, or Ctrl+C.
        tokio::select! {
            _ = config_rx.changed() => {
                current_config = config_rx.borrow().clone();
                tracing::info!("Config changed, restarting monitors…");
            }
            _ = reload_notify.notified() => {
                tracing::info!("IPC reload requested, restarting monitors…");
            }
            _ = tokio::signal::ctrl_c() => {
                tracing::info!("Received Ctrl+C, shutting down…");
                shutdown_token.cancel();

                let grace = tokio::time::Duration::from_secs(10);
                for handle in handles {
                    let _ = tokio::time::timeout(grace, handle).await;
                }
                let _ = tokio::time::timeout(grace, ipc_handle).await;

                tracing::info!("Shutdown complete.");
                return Ok(());
            }
        }

        for handle in handles {
            handle.abort();
        }
    }
}
