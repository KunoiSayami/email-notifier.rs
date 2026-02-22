pub mod account_add;
pub mod account_list;
pub mod account_remove;
pub mod ipc_client;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "email-notifier",
    about = "IMAP-to-Telegram email notification daemon"
)]
pub struct Cli {
    /// Path to the config file
    #[arg(short, long, default_value = "config.toml")]
    pub config: String,

    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Subcommand)]
pub enum Command {
    /// Start the notification daemon
    Run,
    /// Manage email accounts
    Account {
        #[command(subcommand)]
        action: AccountAction,
    },
    /// Send a command to the running daemon via IPC
    Ipc {
        #[command(subcommand)]
        action: IpcAction,
    },
}

#[derive(Subcommand)]
pub enum AccountAction {
    /// Add a new IMAP account
    Add(account_add::AddArgs),
    /// List all configured accounts
    List,
    /// Remove an account by ID
    Remove(account_remove::RemoveArgs),
}

#[derive(Subcommand)]
pub enum IpcAction {
    /// Trigger daemon to reload accounts from database
    Reload,
    /// Check if the daemon is running
    Status,
    /// List all email accounts via daemon
    List,
}
