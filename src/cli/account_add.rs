use anyhow::{Result, bail};
use clap::Args;
use sqlx::SqlitePool;

use email_notifier::db::{self, NewAccount};
use email_notifier::oauth;
use email_notifier::provider;

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

    /// IMAP password or app password (required for password auth, omit for OAuth)
    #[arg(long)]
    pub password: Option<String>,

    /// Authentication method: "password" or "oauth"
    #[arg(long, default_value = "password")]
    pub auth: String,

    /// OAuth refresh token (required when --auth oauth)
    #[arg(long)]
    pub refresh_token: Option<String>,

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

    let (password, auth_method, refresh_token) = match args.auth.as_str() {
        "oauth" | "xoauth2" => {
            let rt = args
                .refresh_token
                .as_deref()
                .ok_or_else(|| anyhow::anyhow!("--refresh-token is required when --auth oauth"))?;
            oauth::OAuthProvider::from_imap_host(&host).ok_or_else(|| {
                anyhow::anyhow!("OAuth is only supported for gmail and outlook providers")
            })?;
            ("", "xoauth2", Some(rt))
        }
        _ => {
            let pw = args.password.as_deref().ok_or_else(|| {
                anyhow::anyhow!("--password is required for password authentication")
            })?;
            (pw, "password", None)
        }
    };

    let id = db::add_account(
        pool,
        &NewAccount {
            label: &args.label,
            imap_host: &host,
            imap_port: port,
            username: &args.username,
            password,
            auth_method,
            refresh_token,
        },
        args.chat_id,
    )
    .await?;

    println!("Account added with ID {id}.");
    println!("  Label:    {}", args.label);
    println!("  Host:     {host}:{port}");
    println!("  Username: {}", args.username);
    println!("  Auth:     {auth_method}");
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
