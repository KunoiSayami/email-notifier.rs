use std::sync::Arc;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

use interprocess::local_socket::{
    GenericNamespaced, ListenerOptions, ToNsName as _,
    traits::tokio::{Listener as _, Stream as _},
};

use teloxide::prelude::*;

use crate::db::{self, NewAccount};
use crate::telegram;

#[derive(Debug, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum IpcRequest {
    Reload,
    Status,
    ListAccounts,
    AddAccount {
        label: String,
        imap_host: String,
        #[serde(default = "default_imap_port")]
        imap_port: u16,
        username: String,
        #[serde(default)]
        password: String,
        chat_id: i64,
        #[serde(default = "default_auth_method")]
        auth_method: String,
        refresh_token: Option<String>,
    },
    RemoveAccount {
        id: i64,
    },
    SendNotification {
        chat_id: i64,
        message: String,
    },
}

fn default_imap_port() -> u16 {
    993
}

fn default_auth_method() -> String {
    "password".to_string()
}

#[derive(Debug, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum IpcResponse {
    Ok { message: String },
    Error { message: String },
    Accounts { accounts: Vec<IpcAccountInfo> },
}

#[derive(Debug, Serialize)]
pub struct IpcAccountInfo {
    id: i64,
    label: String,
    imap_host: String,
    imap_port: u16,
    username: String,
    auth_method: String,
    subscribers: i64,
}

pub async fn run_ipc_server(
    socket_name: String,
    pool: SqlitePool,
    bot: Bot,
    reload_notify: Arc<Notify>,
    cancel: CancellationToken,
) -> Result<()> {
    tracing::info!("IPC server listening on {socket_name}");

    let name = socket_name
        .to_ns_name::<GenericNamespaced>()
        .context("Invalid socket name")?;

    let listener = ListenerOptions::new()
        .name(name)
        .create_tokio()
        .context("Failed to create IPC listener")?;

    loop {
        let conn = tokio::select! {
            result = listener.accept() => result.context("IPC accept failed")?,
            _ = cancel.cancelled() => {
                tracing::info!("IPC server shutting down.");
                return Ok(());
            }
        };
        let pool = pool.clone();
        let bot = bot.clone();
        let reload_notify = reload_notify.clone();

        tokio::spawn(async move {
            let (reader, mut writer) = conn.split();
            let mut lines = BufReader::new(reader).lines();

            while let Ok(Some(line)) = lines.next_line().await {
                let response = match serde_json::from_str::<IpcRequest>(&line) {
                    Ok(req) => handle_request(req, &pool, &bot, &reload_notify).await,
                    Err(e) => IpcResponse::Error {
                        message: format!("Invalid request: {e}"),
                    },
                };

                let mut resp_json = serde_json::to_string(&response).unwrap_or_else(|e| {
                    format!(r#"{{"status":"error","message":"Serialization failed: {e}"}}"#)
                });
                resp_json.push('\n');

                if writer.write_all(resp_json.as_bytes()).await.is_err() {
                    break;
                }
            }
        });
    }
}

async fn handle_request(
    req: IpcRequest,
    pool: &SqlitePool,
    bot: &Bot,
    reload_notify: &Notify,
) -> IpcResponse {
    match req {
        IpcRequest::Reload => {
            reload_notify.notify_one();
            IpcResponse::Ok {
                message: "Reload triggered".into(),
            }
        }
        IpcRequest::Status => IpcResponse::Ok {
            message: "Running".into(),
        },
        IpcRequest::ListAccounts => match db::list_accounts(pool).await {
            Ok(accounts) => {
                let mut infos = Vec::with_capacity(accounts.len());
                for acct in &accounts {
                    let subscribers = db::get_subscriber_count(pool, acct.id()).await.unwrap_or(0);
                    infos.push(IpcAccountInfo {
                        id: acct.id(),
                        label: acct.label().to_owned(),
                        imap_host: acct.imap_host().to_owned(),
                        imap_port: acct.imap_port(),
                        username: acct.username().to_owned(),
                        auth_method: acct.auth_method().to_owned(),
                        subscribers,
                    });
                }
                IpcResponse::Accounts { accounts: infos }
            }
            Err(e) => IpcResponse::Error {
                message: format!("Failed to list accounts: {e}"),
            },
        },
        IpcRequest::AddAccount {
            label,
            imap_host,
            imap_port,
            username,
            password,
            chat_id,
            auth_method,
            refresh_token,
        } => {
            let account = NewAccount {
                label: &label,
                imap_host: &imap_host,
                imap_port,
                username: &username,
                password: &password,
                auth_method: &auth_method,
                refresh_token: refresh_token.as_deref(),
            };
            match db::add_account(pool, &account, chat_id).await {
                Ok(id) => IpcResponse::Ok {
                    message: format!("Account added with id {id}"),
                },
                Err(e) => IpcResponse::Error {
                    message: format!("Failed to add account: {e}"),
                },
            }
        }
        IpcRequest::RemoveAccount { id } => match db::remove_account(pool, id).await {
            Ok(true) => IpcResponse::Ok {
                message: format!("Account {id} removed"),
            },
            Ok(false) => IpcResponse::Error {
                message: format!("Account {id} not found"),
            },
            Err(e) => IpcResponse::Error {
                message: format!("Failed to remove account: {e}"),
            },
        },
        IpcRequest::SendNotification { chat_id, message } => {
            match telegram::send_notification(bot, chat_id, &message).await {
                Ok(()) => IpcResponse::Ok {
                    message: "Notification sent".into(),
                },
                Err(e) => IpcResponse::Error {
                    message: format!("Failed to send notification: {e}"),
                },
            }
        }
    }
}
