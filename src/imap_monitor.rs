use std::time::Duration;

use anyhow::{Context, Result};
use async_imap::extensions::idle::IdleResponse;
use async_tls::TlsConnector;
use futures::TryStreamExt;
use sqlx::SqlitePool;
use teloxide::prelude::*;
use tokio::time::sleep;
use tokio_util::compat::TokioAsyncReadCompatExt as _;
use tokio_util::sync::CancellationToken;

use crate::{
    config::OAuthConfig,
    db::{self, EmailAccount},
    email_formatter::{format_telegram_message, parse_email},
    oauth,
    telegram::send_notification,
};

const IDLE_TIMEOUT: Duration = Duration::from_secs(10 * 60); // 25 min (RFC 2177 recommends <29 min)
const RECONNECT_BASE_DELAY: Duration = Duration::from_secs(5);
const RECONNECT_MAX_DELAY: Duration = Duration::from_secs(5 * 60);

type ImapSession = async_imap::Session<
    async_tls::client::TlsStream<tokio_util::compat::Compat<tokio::net::TcpStream>>,
>;

pub async fn monitor_account(
    account: EmailAccount,
    pool: SqlitePool,
    bot: Bot,
    oauth_config: OAuthConfig,
    cancel: CancellationToken,
) {
    let mut backoff = RECONNECT_BASE_DELAY;

    loop {
        let result = tokio::select! {
            r = async {
                let ret = run_imap_session(&account, &pool, &bot, &oauth_config).await;
                tracing::debug!("{ret:?}");
                ret
            } => Some(r),
            _ = cancel.cancelled() => None,
        };

        match result {
            None => {
                tracing::info!(
                    "[{}] Shutdown requested, stopping monitor.",
                    account.label()
                );
                break;
            }
            Some(Ok(())) => {
                tracing::info!("[{}] IMAP session ended, reconnecting…", account.label());
                backoff = RECONNECT_BASE_DELAY;
            }
            Some(Err(e)) => {
                tracing::error!("[{}] IMAP error: {e:#}", account.label());
                tracing::info!(
                    "[{}] Reconnecting in {}s…",
                    account.label(),
                    backoff.as_secs()
                );
                tokio::select! {
                    _ = sleep(backoff) => {}
                    _ = cancel.cancelled() => {
                        tracing::info!(
                            "[{}] Shutdown requested during backoff, stopping.",
                            account.label()
                        );
                        break;
                    }
                }
                backoff = (backoff * 2).min(RECONNECT_MAX_DELAY);
            }
        }
    }
}

async fn run_imap_session(
    account: &EmailAccount,
    pool: &SqlitePool,
    bot: &Bot,
    oauth_config: &OAuthConfig,
) -> Result<()> {
    tracing::info!(
        "[{}] Connecting to {}:{}…",
        account.label(),
        account.imap_host(),
        account.imap_port()
    );

    let tls = TlsConnector::default();
    let tcp_stream = tokio::net::TcpStream::connect((account.imap_host(), account.imap_port()))
        .await
        .with_context(|| {
            format!(
                "TCP connect to {}:{} failed",
                account.imap_host(),
                account.imap_port()
            )
        })?;

    // Wrap tokio TcpStream with compat layer so it implements futures::AsyncRead/Write
    let compat_stream = tcp_stream.compat();

    let tls_stream = tls
        .connect(account.imap_host(), compat_stream)
        .await
        .context("TLS handshake failed")?;

    let mut client = async_imap::Client::new(tls_stream);

    // Client::new does not consume the server greeting. We must read it
    // before calling authenticate(), otherwise the greeting gets mixed into
    // the SASL handshake and causes a deadlock.
    client
        .read_response()
        .await
        .context("Failed to read server greeting")?
        .context("Connection closed before greeting")?;

    let mut session = if account.is_oauth() {
        let provider = oauth::OAuthProvider::from_imap_host(account.imap_host())
            .context("Unknown OAuth provider for IMAP host")?;
        let credentials = oauth::get_credentials(oauth_config, provider)?;
        let refresh_token = account
            .refresh_token()
            .context("OAuth account missing refresh_token")?;

        let access_token =
            oauth::refresh_access_token(provider, credentials, refresh_token).await?;
        db::update_access_token(pool, account.id(), &access_token).await?;

        let mut authenticator = oauth::XOAuth2::new(account.username().to_owned(), access_token);
        client
            .authenticate("XOAUTH2", &mut authenticator)
            .await
            .map_err(|(e, _)| anyhow::anyhow!("XOAUTH2 auth failed: {e}"))?
    } else {
        client
            .login(account.username(), account.password())
            .await
            .map_err(|(e, _)| anyhow::anyhow!("IMAP login failed: {e}"))?
    };

    tracing::info!("[{}] Logged in.", account.label());

    session
        .select("INBOX")
        .await
        .context("Failed to SELECT INBOX")?;

    // On first-ever connect for this account, mark all existing UIDs as seen
    // to avoid spamming. On reconnects, skip so we pick up emails that arrived
    // while disconnected.
    if !db::has_seen_uids(pool, account.id()).await? {
        seed_seen_uids(account, pool, &mut session).await?;
    }

    loop {
        // Enter IDLE
        let mut idle = session.idle();
        idle.init().await.context("IDLE init failed")?;

        let (idle_wait, _stop_source) = idle.wait_with_timeout(IDLE_TIMEOUT);
        let idle_response = idle_wait.await.context("IDLE wait failed")?;

        session = idle.done().await.context("IDLE done failed")?;

        match idle_response {
            IdleResponse::NewData(_) | IdleResponse::Timeout => {
                process_new_emails(account, pool, bot, &mut session).await?;
            }
            IdleResponse::ManualInterrupt => break,
        }
    }

    session.logout().await.ok();
    Ok(())
}

async fn seed_seen_uids(
    account: &EmailAccount,
    pool: &SqlitePool,
    session: &mut ImapSession,
) -> Result<()> {
    let uids = fetch_all_uids(session).await?;
    let uid_strings: Vec<String> = uids.into_iter().map(|u| u.to_string()).collect();
    db::mark_all_uids_seen(pool, account.id(), &uid_strings).await?;
    tracing::info!(
        "[{}] Seeded {} existing UIDs as seen.",
        account.label(),
        uid_strings.len()
    );
    Ok(())
}

async fn fetch_all_uids(session: &mut ImapSession) -> Result<Vec<u32>> {
    let uids = session
        .uid_search("ALL")
        .await
        .context("UID SEARCH ALL failed")?;
    Ok(uids.into_iter().collect())
}

async fn process_new_emails(
    account: &EmailAccount,
    pool: &SqlitePool,
    bot: &Bot,
    session: &mut ImapSession,
) -> Result<()> {
    let unseen_uids: Vec<u32> = session
        .uid_search("UNSEEN")
        .await
        .context("UID SEARCH UNSEEN failed")?
        .into_iter()
        .collect();

    for uid in unseen_uids {
        let uid_str = uid.to_string();

        if db::is_uid_seen(pool, account.id(), &uid_str).await? {
            continue;
        }

        match fetch_and_notify(account, pool, bot, session, uid).await {
            Ok(()) => {
                db::mark_uid_seen(pool, account.id(), &uid_str).await?;
            }
            Err(e) => {
                tracing::warn!("[{}] Failed to process UID {uid}: {e:#}", account.label());
            }
        }
    }

    Ok(())
}

async fn fetch_and_notify(
    account: &EmailAccount,
    pool: &SqlitePool,
    bot: &Bot,
    session: &mut ImapSession,
    uid: u32,
) -> Result<()> {
    let fetch_seq = uid.to_string();
    let mut messages = session
        .uid_fetch(&fetch_seq, "BODY.PEEK[]")
        .await
        .context("UID FETCH failed")?;

    while let Some(fetch) = messages.try_next().await? {
        let raw = fetch.body().context("Message has no body")?;
        let parsed = parse_email(raw)?;
        let text = format_telegram_message(account.label(), &parsed);

        let chat_ids = db::get_subscriber_chat_ids(pool, account.id()).await?;
        for chat_id in &chat_ids {
            if let Err(e) = send_notification(bot, *chat_id, &text).await {
                tracing::warn!(
                    "[{}] Failed to notify chat {chat_id}: {e:#}",
                    account.label()
                );
            }
        }

        tracing::info!(
            "[{}] Notified {} subscriber(s) about UID {uid}: \"{}\"",
            account.label(),
            chat_ids.len(),
            parsed.subject()
        );
    }

    Ok(())
}
