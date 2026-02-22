use anyhow::Result;
use sqlx::SqlitePool;
use teloxide::{prelude::*, types::ParseMode, utils::command::BotCommands};

use crate::{
    db::{self, NewAccount},
    email_formatter::escape_html,
    provider,
    telegram::send_notification,
};

#[derive(BotCommands, Clone)]
#[command(rename_rule = "lowercase")]
enum Command {
    Start,
    Id,
    Providers,
    #[command(parse_with = "split")]
    Add {
        label: String,
        provider_or_host: String,
        username: String,
        password: String,
    },
    List,
    #[command(parse_with = "split")]
    Remove {
        id: i64,
    },
    #[command(parse_with = "split")]
    Allow {
        target_chat_id: i64,
    },
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
    let chat_id = msg.chat.id.0;

    match cmd {
        Command::Start => handle_start(&bot, &msg, &pool, admin_chat_id).await?,
        Command::Id => handle_id(&bot, &msg).await?,
        Command::Providers => handle_providers(&bot, &msg).await?,
        Command::Add {
            label,
            provider_or_host,
            username,
            password,
        } => {
            if !is_authorized(&pool, chat_id, admin_chat_id).await? {
                reply_unauthorized(&bot, &msg).await?;
                return Ok(());
            }
            handle_add(
                &bot,
                &msg,
                &pool,
                &label,
                &provider_or_host,
                &username,
                &password,
            )
            .await?;
        }
        Command::List => {
            if !is_authorized(&pool, chat_id, admin_chat_id).await? {
                reply_unauthorized(&bot, &msg).await?;
                return Ok(());
            }
            handle_list(&bot, &msg, &pool).await?;
        }
        Command::Remove { id } => {
            if !is_authorized(&pool, chat_id, admin_chat_id).await? {
                reply_unauthorized(&bot, &msg).await?;
                return Ok(());
            }
            handle_remove(&bot, &msg, &pool, id).await?;
        }
        Command::Allow { target_chat_id } => {
            if chat_id != admin_chat_id {
                reply_unauthorized(&bot, &msg).await?;
                return Ok(());
            }
            handle_allow(&bot, &msg, &pool, target_chat_id).await?;
        }
    }

    Ok(())
}

async fn is_authorized(pool: &SqlitePool, chat_id: i64, admin_chat_id: i64) -> Result<bool> {
    if chat_id == admin_chat_id {
        return Ok(true);
    }
    db::is_user_allowed(pool, chat_id).await
}

async fn reply_unauthorized(bot: &Bot, msg: &Message) -> Result<()> {
    bot.send_message(
        msg.chat.id,
        "Not authorized. Send /id and ask the admin to /allow your ID.",
    )
    .await?;
    Ok(())
}

async fn handle_start(
    bot: &Bot,
    msg: &Message,
    pool: &SqlitePool,
    admin_chat_id: i64,
) -> Result<()> {
    let chat_id = msg.chat.id.0;
    let user = msg.from.as_ref();

    let username = user.and_then(|u| u.username.as_deref());
    let first_name = user.map(|u| u.first_name.as_str());
    let last_name = user.and_then(|u| u.last_name.as_deref());

    bot.send_message(msg.chat.id, "Welcome! The bot admin has been notified.")
        .await?;

    let is_new = db::register_bot_user(pool, chat_id, username, first_name, last_name).await?;

    if is_new {
        let notification = format_admin_notification(chat_id, username, first_name, last_name);
        send_notification(bot, admin_chat_id, &notification).await?;
        tracing::info!("New user started bot: chat_id={chat_id}, username={username:?}");
    } else {
        tracing::debug!("Existing user re-started bot: chat_id={chat_id}");
    }

    Ok(())
}

async fn handle_id(bot: &Bot, msg: &Message) -> Result<()> {
    let chat_id = msg.chat.id.0;
    bot.send_message(msg.chat.id, format!("Your chat ID: <code>{chat_id}</code>"))
        .parse_mode(ParseMode::Html)
        .await?;
    Ok(())
}

async fn handle_providers(bot: &Bot, msg: &Message) -> Result<()> {
    let providers = provider::all();
    let mut text = String::from("<b>Built-in providers:</b>\n");
    for (name, p) in providers {
        text.push_str(&format!(
            "\n<code>{name}</code> — {}:{}",
            escape_html(p.imap_host()),
            p.imap_port()
        ));
    }
    bot.send_message(msg.chat.id, text)
        .parse_mode(ParseMode::Html)
        .await?;
    Ok(())
}

async fn handle_add(
    bot: &Bot,
    msg: &Message,
    pool: &SqlitePool,
    label: &str,
    provider_or_host: &str,
    username: &str,
    password: &str,
) -> Result<()> {
    let chat_id = msg.chat.id.0;

    let (imap_host, imap_port) = if let Some(p) = provider::lookup(provider_or_host) {
        (p.imap_host().to_owned(), p.imap_port())
    } else {
        parse_host_port(provider_or_host)
    };

    let account = NewAccount {
        label,
        imap_host: &imap_host,
        imap_port,
        username,
        password,
    };

    let id = db::add_account(pool, &account, chat_id).await?;
    let text = format!(
        "Account added (id: {id}).\n<code>{}</code> → {}:{}",
        escape_html(label),
        escape_html(&imap_host),
        imap_port
    );
    bot.send_message(msg.chat.id, text)
        .parse_mode(ParseMode::Html)
        .await?;
    tracing::info!("Account added via bot: id={id}, label={label}, chat_id={chat_id}");
    Ok(())
}

fn parse_host_port(input: &str) -> (String, u16) {
    match input.rsplit_once(':') {
        Some((host, port_str)) => match port_str.parse::<u16>() {
            Ok(port) => (host.to_owned(), port),
            Err(_) => (input.to_owned(), 993),
        },
        None => (input.to_owned(), 993),
    }
}

async fn handle_list(bot: &Bot, msg: &Message, pool: &SqlitePool) -> Result<()> {
    let chat_id = msg.chat.id.0;
    let accounts = db::list_accounts_by_chat_id(pool, chat_id).await?;

    if accounts.is_empty() {
        bot.send_message(msg.chat.id, "No accounts found.").await?;
        return Ok(());
    }

    let mut text = String::from("<b>Your accounts:</b>\n");
    for account in &accounts {
        text.push_str(&format!(
            "\n[{}] <code>{}</code> — {} ({})",
            account.id(),
            escape_html(account.label()),
            escape_html(account.username()),
            escape_html(account.imap_host()),
        ));
    }
    bot.send_message(msg.chat.id, text)
        .parse_mode(ParseMode::Html)
        .await?;
    Ok(())
}

async fn handle_remove(bot: &Bot, msg: &Message, pool: &SqlitePool, id: i64) -> Result<()> {
    let chat_id = msg.chat.id.0;
    let removed = db::remove_account_by_id_and_chat_id(pool, id, chat_id).await?;

    if removed {
        bot.send_message(msg.chat.id, format!("Account {id} removed."))
            .await?;
        tracing::info!("Account removed via bot: id={id}, chat_id={chat_id}");
    } else {
        bot.send_message(msg.chat.id, format!("Account {id} not found or not yours."))
            .await?;
    }
    Ok(())
}

async fn handle_allow(
    bot: &Bot,
    msg: &Message,
    pool: &SqlitePool,
    target_chat_id: i64,
) -> Result<()> {
    let allowed = db::allow_user(pool, target_chat_id).await?;

    if allowed {
        bot.send_message(
            msg.chat.id,
            format!("User <code>{target_chat_id}</code> is now allowed."),
        )
        .parse_mode(ParseMode::Html)
        .await?;
        tracing::info!("Admin allowed user: chat_id={target_chat_id}");
    } else {
        bot.send_message(
            msg.chat.id,
            format!("User <code>{target_chat_id}</code> not found. They need to /start first."),
        )
        .parse_mode(ParseMode::Html)
        .await?;
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
