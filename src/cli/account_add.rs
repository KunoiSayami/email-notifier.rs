use anyhow::{Result, bail};
use clap::Args;
use sqlx::SqlitePool;

use crate::db::{self, NewAccount};
use crate::provider;

#[derive(Args)]
pub struct AddArgs {
    /// Human-readable label for this account
    #[arg(long)]
    pub label: String,

    /// Use a built-in provider (gmail, outlook, yahoo, icloud, fastmail)
    #[arg(long)]
    pub provider: Option<String>,

    /// IMAP server hostname (overrides --provider)
    #[arg(long)]
    pub host: Option<String>,

    /// IMAP server port (overrides --provider)
    #[arg(long)]
    pub port: Option<u16>,

    /// Email address / IMAP login username
    #[arg(long)]
    pub username: String,

    /// IMAP password or app password
    #[arg(long)]
    pub password: String,

    /// Telegram chat ID to send notifications to
    #[arg(long)]
    pub chat_id: i64,

    /// List available built-in providers and exit
    #[arg(long)]
    pub list_providers: bool,
}

pub async fn run(pool: &SqlitePool, args: &AddArgs) -> Result<()> {
    if args.list_providers {
        print_providers();
        return Ok(());
    }

    let (host, port) = resolve_host_port(args)?;

    let id = db::add_account(
        pool,
        &NewAccount {
            label: &args.label,
            imap_host: &host,
            imap_port: port,
            username: &args.username,
            password: &args.password,
        },
        args.chat_id,
    )
    .await?;

    println!("Account added with ID {id}.");
    println!("  Label:    {}", args.label);
    println!("  Host:     {host}:{port}");
    println!("  Username: {}", args.username);
    println!("  Chat ID:  {}", args.chat_id);

    Ok(())
}

fn resolve_host_port(args: &AddArgs) -> Result<(String, u16)> {
    // Explicit --host takes priority
    if let Some(ref host) = args.host {
        let port = args.port.unwrap_or(993);
        return Ok((host.clone(), port));
    }

    // Try provider lookup
    if let Some(ref name) = args.provider {
        let p = provider::lookup(name).ok_or_else(|| {
            anyhow::anyhow!(
                "Unknown provider '{name}'. Use --list-providers to see available options."
            )
        })?;
        let host = args
            .host
            .clone()
            .unwrap_or_else(|| p.imap_host().to_owned());
        let port = args.port.unwrap_or(p.imap_port());
        return Ok((host, port));
    }

    // Neither --provider nor --host given
    bail!(
        "Must specify --provider or --host.\n\nAvailable providers:\n{}",
        format_provider_list()
    );
}

fn print_providers() {
    println!("Available providers:\n");
    println!("  {:<12} {:<28} {}", "Name", "IMAP Host", "Port");
    println!("  {:<12} {:<28} {}", "----", "---------", "----");
    for (name, p) in provider::all() {
        println!("  {:<12} {:<28} {}", name, p.imap_host(), p.imap_port());
    }
    println!("\nUsage: email-notifier account add --provider <name> ...");
}

fn format_provider_list() -> String {
    provider::all()
        .iter()
        .map(|(name, _)| format!("  - {name}"))
        .collect::<Vec<_>>()
        .join("\n")
}
