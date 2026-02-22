use anyhow::Result;
use sqlx::SqlitePool;
use teloxide::{prelude::*, utils::command::BotCommands};

use crate::{db, email_formatter::escape_html, telegram::send_notification};

#[derive(BotCommands, Clone)]
#[command(rename_rule = "lowercase")]
enum Command {
    Start,
}

pub async fn run_command_handler(bot: Bot, pool: SqlitePool, admin_chat_id: i64) {
    let handler = Update::filter_message()
        .filter_command::<Command>()
        .endpoint(handle_command);

    Dispatcher::builder(bot, handler)
        .dependencies(dptree::deps![pool, admin_chat_id])
        .default_handler(|_upd| async {})
        .error_handler(LoggingErrorHandler::with_custom_text(
            "Error in command handler",
        ))
        .build()
        .dispatch()
        .await;
}

async fn handle_command(
    bot: Bot,
    msg: Message,
    cmd: Command,
    pool: SqlitePool,
    admin_chat_id: i64,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    match cmd {
        Command::Start => handle_start(bot, msg, pool, admin_chat_id).await?,
    }
    Ok(())
}

async fn handle_start(bot: Bot, msg: Message, pool: SqlitePool, admin_chat_id: i64) -> Result<()> {
    let chat_id = msg.chat.id.0;
    let user = msg.from.as_ref();

    let username = user.and_then(|u| u.username.as_deref());
    let first_name = user.map(|u| u.first_name.as_str());
    let last_name = user.and_then(|u| u.last_name.as_deref());

    bot.send_message(msg.chat.id, "Welcome! The bot admin has been notified.")
        .await?;

    let is_new = db::register_bot_user(&pool, chat_id, username, first_name, last_name).await?;

    if is_new {
        let notification = format_admin_notification(chat_id, username, first_name, last_name);
        send_notification(&bot, admin_chat_id, &notification).await?;
        tracing::info!("New user started bot: chat_id={chat_id}, username={username:?}");
    } else {
        tracing::debug!("Existing user re-started bot: chat_id={chat_id}");
    }

    Ok(())
}

fn format_admin_notification(
    chat_id: i64,
    username: Option<&str>,
    first_name: Option<&str>,
    last_name: Option<&str>,
) -> String {
    let mut msg = String::from("👤 <b>New user started the bot</b>\n\n");

    if let Some(first) = first_name {
        msg.push_str(&format!("<b>Name:</b> {}", escape_html(first)));
        if let Some(last) = last_name {
            msg.push_str(&format!(" {}", escape_html(last)));
        }
        msg.push('\n');
    }

    if let Some(uname) = username {
        msg.push_str(&format!("<b>Username:</b> @{}\n", escape_html(uname)));
    }

    msg.push_str(&format!("<b>Chat ID:</b> <code>{chat_id}</code>"));

    msg
}
