use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use std::collections::HashMap;

use anyhow::{Context, Result};
use axum::{
    Router,
    extract::{Query, State},
    response::Html,
    routing::get,
};
use clap::Parser;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

use interprocess::local_socket::{
    GenericNamespaced, ToNsName as _, tokio::Stream, traits::tokio::Stream as _,
};

use email_notifier::{config, email_formatter, oauth, provider};

#[derive(Parser)]
#[command(name = "email-notifier-oauth", about = "OAuth callback server")]
struct Cli {
    /// Path to the config file
    #[arg(short, long, default_value = "config.toml")]
    config: PathBuf,

    /// Address to listen on
    #[arg(short, long, default_value = "127.0.0.1:27000")]
    listen: SocketAddr,
}

struct AppState {
    config: config::Config,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    let cfg = config::load_config(&cli.config)
        .with_context(|| format!("Failed to load config from '{}'", cli.config.display()))?;

    // Initialize tracing
    let filter = tracing_subscriber::EnvFilter::try_new(&cfg.log().level())
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    tracing_subscriber::fmt().with_env_filter(filter).init();

    let state = Arc::new(AppState { config: cfg });

    let app = Router::new()
        .route("/callback", get(oauth_callback))
        .route("/health", get(health))
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(cli.listen).await?;
    tracing::info!("OAuth callback server listening on {}", cli.listen);

    axum::serve(listener, app).await?;

    Ok(())
}

async fn health() -> &'static str {
    "ok"
}

async fn oauth_callback(
    State(state): State<Arc<AppState>>,
    Query(raw_params): Query<HashMap<String, String>>,
) -> Html<String> {
    tracing::trace!(?raw_params, "Received OAuth callback query params");

    let (code, oauth_state) = match (raw_params.get("code"), raw_params.get("state")) {
        (Some(c), Some(s)) => (c.clone(), s.clone()),
        _ => {
            tracing::error!(?raw_params, "Missing required OAuth callback params");
            return Html(format!(
                "<!DOCTYPE html><html><body>\
                 <h1>Error</h1><p>Missing required callback parameters.</p>\
                 <p>Received params: <code>{}</code></p>\
                 <p>Please try again with /addoauth in Telegram.</p>\
                 </body></html>",
                email_formatter::escape_html(&format!("{raw_params:?}")),
            ));
        }
    };

    match handle_callback(&state, &code, &oauth_state).await {
        Ok(msg) => Html(format!(
            "<!DOCTYPE html><html><body>\
             <h1>Success</h1><p>{msg}</p>\
             <p>You can close this window.</p>\
             </body></html>"
        )),
        Err(e) => {
            tracing::error!("OAuth callback error: {e:#}");
            Html(format!(
                "<!DOCTYPE html><html><body>\
                 <h1>Error</h1><p>{}</p>\
                 <p>Please try again with /addoauth in Telegram.</p>\
                 </body></html>",
                email_formatter::escape_html(&e.to_string()),
            ))
        }
    }
}

async fn handle_callback(state: &AppState, code: &str, state_param: &str) -> Result<String> {
    // Decode session data from state parameter.
    let oauth_state = oauth::OAuthState::decode(state_param)?;
    tracing::trace!(?oauth_state, "Decoded OAuth state");

    // Resolve provider.
    let oauth_provider = oauth::OAuthProvider::from_name(&oauth_state.provider_name)
        .context("Unknown OAuth provider in state")?;

    let credentials = oauth::get_credentials(state.config.oauth(), oauth_provider)?;
    let redirect_uri = credentials
        .redirect_uri()
        .context("No redirect_uri in config")?;
    tracing::trace!(%redirect_uri, provider = %oauth_provider.name(), "Exchanging authorization code");

    // Exchange authorization code for tokens.
    let (_access_token, refresh_token) =
        oauth::exchange_authorization_code(oauth_provider, credentials, redirect_uri, code).await?;
    tracing::trace!("Token exchange completed successfully");

    // Look up IMAP details.
    let p = provider::lookup_by_oauth_provider(oauth_provider);

    // Send IPC AddAccount to the daemon.
    let add_request = serde_json::json!({
        "cmd": "add_account",
        "label": oauth_state.label,
        "imap_host": p.imap_host(),
        "imap_port": p.imap_port(),
        "username": oauth_state.username,
        "password": "",
        "chat_id": oauth_state.chat_id,
        "auth_method": "xoauth2",
        "refresh_token": refresh_token,
    });

    let socket_name = state.config.ipc().socket_name();

    // Add the account.
    let resp = send_ipc_request(socket_name, &add_request.to_string()).await?;
    tracing::info!("IPC AddAccount response: {resp}");

    // Trigger reload so the daemon picks up the new account.
    let reload_resp = send_ipc_request(socket_name, r#"{"cmd":"reload"}"#).await?;
    tracing::info!("IPC Reload response: {reload_resp}");

    // Notify the user in Telegram.
    let notification = format!(
        "OAuth account added.\n<code>{}</code> via {}",
        email_formatter::escape_html(&oauth_state.label),
        oauth_provider.name(),
    );
    let notify_request = serde_json::json!({
        "cmd": "send_notification",
        "chat_id": oauth_state.chat_id,
        "message": notification,
    });
    let notify_resp = send_ipc_request(socket_name, &notify_request.to_string()).await?;
    tracing::info!("IPC SendNotification response: {notify_resp}");

    Ok(format!(
        "Account \"{}\" added via {}. Check Telegram for confirmation.",
        email_formatter::escape_html(&oauth_state.label),
        oauth_provider.name(),
    ))
}

/// Send a single IPC request and return the response.
async fn send_ipc_request(socket_name: &str, request: &str) -> Result<String> {
    let name = socket_name
        .to_ns_name::<GenericNamespaced>()
        .context("Invalid socket name")?;

    let conn = Stream::connect(name)
        .await
        .context("Failed to connect to daemon IPC. Is it running?")?;

    let (reader, mut writer) = conn.split();

    writer
        .write_all(format!("{request}\n").as_bytes())
        .await
        .context("Failed to send IPC request")?;

    let mut lines = BufReader::new(reader).lines();
    let line = lines
        .next_line()
        .await
        .context("Failed to read IPC response")?
        .context("Empty IPC response")?;

    Ok(line)
}
