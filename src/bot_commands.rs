use anyhow::Result;
use sqlx::SqlitePool;
use teloxide::{
    prelude::*,
    types::{
        CallbackQuery, InlineKeyboardButton, InlineKeyboardMarkup, LinkPreviewOptions, ParseMode,
    },
    utils::command::BotCommands,
};
use tokio_util::sync::CancellationToken;

use crate::{
    config::OAuthConfig,
    db::{self, NewAccount},
    email_formatter::{assemble_message, escape_html, make_preview},
    oauth, provider,
    telegram::send_notification,
};

#[derive(BotCommands, Clone)]
#[command(rename_rule = "lowercase")]
enum Command {
    #[command(description = "Register as a new user")]
    Start,
    #[command(description = "Show available commands")]
    Help,
    #[command(description = "Show your chat ID")]
    Id,
    #[command(description = "List supported email providers")]
    Providers,
    #[command(description = "Add email account (password auth)")]
    #[command(parse_with = "split")]
    Add {
        label: String,
        provider_or_host: String,
        username: String,
        password: String,
    },
    #[command(description = "Add email account (OAuth)")]
    #[command(parse_with = "split")]
    Addoauth {
        label: String,
        provider: String,
        username: String,
    },
    #[command(description = "List your email accounts")]
    List,
    #[command(description = "Remove an email account")]
    #[command(parse_with = "split")]
    Remove { id: i64 },
    #[command(description = "Approve a user (admin only)")]
    #[command(parse_with = "split")]
    Allow { target_chat_id: i64 },
}

pub async fn run_command_handler(
    bot: Bot,
    pool: SqlitePool,
    admin_chat_id: i64,
    bypass_registration: bool,
    oauth_config: OAuthConfig,
    cancel: CancellationToken,
) {
    let handler = dptree::entry()
        .branch(
            Update::filter_message()
                .filter_command::<Command>()
                .endpoint(handle_command),
        )
        .branch(
            Update::filter_message()
                .filter(|msg: Message| msg.text().is_some_and(|text| text.starts_with('/')))
                .endpoint(handle_unknown_command),
        )
        .branch(Update::filter_callback_query().endpoint(handle_callback_query));

    let mut dispatcher = Dispatcher::builder(bot, handler)
        .dependencies(dptree::deps![
            pool,
            admin_chat_id,
            bypass_registration,
            oauth_config
        ])
        .default_handler(|_upd| async {})
        .error_handler(LoggingErrorHandler::with_custom_text(
            "Error in command handler",
        ))
        .build();

    // Bridge CancellationToken to teloxide's ShutdownToken.
    let teloxide_shutdown = dispatcher.shutdown_token();
    tokio::spawn(async move {
        cancel.cancelled().await;
        if let Ok(fut) = teloxide_shutdown.shutdown() {
            fut.await;
        }
    });

    dispatcher.dispatch().await;
}

async fn handle_command(
    bot: Bot,
    msg: Message,
    cmd: Command,
    pool: SqlitePool,
    admin_chat_id: i64,
    bypass_registration: bool,
    oauth_config: OAuthConfig,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let chat_id = msg.chat.id.0;

    match cmd {
        Command::Start => handle_start(&bot, &msg, &pool, admin_chat_id).await?,
        Command::Help => handle_help(&bot, &msg).await?,
        Command::Id => handle_id(&bot, &msg).await?,
        Command::Providers => handle_providers(&bot, &msg).await?,
        Command::Add {
            label,
            provider_or_host,
            username,
            password,
        } => {
            if !is_authorized(&pool, chat_id, admin_chat_id, bypass_registration).await? {
                reply_unauthorized(&bot, &msg, &pool, admin_chat_id).await?;
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
        Command::Addoauth {
            label,
            provider,
            username,
        } => {
            if !is_authorized(&pool, chat_id, admin_chat_id, bypass_registration).await? {
                reply_unauthorized(&bot, &msg, &pool, admin_chat_id).await?;
                return Ok(());
            }
            handle_add_oauth(&bot, &msg, &oauth_config, &label, &provider, &username).await?;
        }
        Command::List => {
            if !is_authorized(&pool, chat_id, admin_chat_id, bypass_registration).await? {
                //reply_unauthorized(&bot, &msg, &pool, admin_chat_id).await?;
                return Ok(());
            }
            handle_list(&bot, &msg, &pool).await?;
        }
        Command::Remove { id } => {
            if !is_authorized(&pool, chat_id, admin_chat_id, bypass_registration).await? {
                //reply_unauthorized(&bot, &msg, &pool, admin_chat_id).await?;
                return Ok(());
            }
            handle_remove(&bot, &msg, &pool, id).await?;
        }
        Command::Allow { target_chat_id } => {
            if chat_id != admin_chat_id {
                //reply_unauthorized(&bot, &msg, &pool, admin_chat_id).await?;
                return Ok(());
            }
            handle_allow(&bot, &msg, &pool, target_chat_id).await?;
        }
    }

    Ok(())
}

async fn is_authorized(
    pool: &SqlitePool,
    chat_id: i64,
    admin_chat_id: i64,
    bypass_registration: bool,
) -> Result<bool> {
    if bypass_registration || chat_id == admin_chat_id {
        return Ok(true);
    }
    db::is_user_allowed(pool, chat_id).await
}

async fn reply_unauthorized(
    bot: &Bot,
    msg: &Message,
    pool: &SqlitePool,
    admin_chat_id: i64,
) -> Result<()> {
    let chat_id = msg.chat.id.0;

    bot.send_message(
        msg.chat.id,
        "Not authorized. The admin has been notified of your request.",
    )
    .await?;

    // Only notify admin if the user has /start-ed and hasn't been notified yet.
    if let Some(user) = db::get_bot_user(pool, chat_id).await? {
        if user.notified_admin() {
            return Ok(());
        }

        let text = format_auth_request(
            chat_id,
            user.username(),
            user.first_name(),
            user.last_name(),
        );
        let keyboard = InlineKeyboardMarkup::new(vec![vec![
            InlineKeyboardButton::callback("✅ Allow", format!("allow:{chat_id}")),
            InlineKeyboardButton::callback("❌ Deny", format!("deny:{chat_id}")),
        ]]);

        bot.send_message(ChatId(admin_chat_id), text)
            .parse_mode(ParseMode::Html)
            .reply_markup(keyboard)
            .await?;

        db::mark_admin_notified(pool, chat_id).await?;
    }

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

    bot.send_message(
        msg.chat.id,
        "Welcome! Use /help to show more detailed usage",
    )
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

async fn handle_unknown_command(
    bot: Bot,
    msg: Message,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    bot.send_message(
        msg.chat.id,
        "Unknown command. Use /help to see available commands.",
    )
    .await?;
    Ok(())
}

async fn handle_help(bot: &Bot, msg: &Message) -> Result<()> {
    let text = concat!(
        "<b>Available commands:</b>\n",
        "\n/start — Register as a new user",
        "\n/help — Show this help message",
        "\n/id — Show your chat ID",
        "\n/providers — List supported email providers",
        "\n/list — List your email accounts",
        "\n",
        "\n/add <code>&lt;label&gt;</code> <code>&lt;provider_or_host&gt;</code> <code>&lt;username&gt;</code> <code>&lt;password&gt;</code>",
        "\n  Add an email account with password authentication.",
        "\n  <code>label</code> — A name for this account (e.g. <i>work</i>)",
        "\n  <code>provider_or_host</code> — Built-in provider name (see /providers) or <i>host:port</i>",
        "\n  <code>username</code> — Email address",
        "\n  <code>password</code> — Email password or app-specific password",
        "\n",
        "\n/addoauth <code>&lt;label&gt;</code> <code>&lt;provider&gt;</code> <code>&lt;username&gt;</code>",
        "\n  Add an email account with OAuth authentication.",
        "\n  <code>label</code> — A name for this account",
        "\n  <code>provider</code> — Built-in provider name (e.g. <i>gmail</i>, <i>outlook</i>)",
        "\n  <code>username</code> — Email address",
        "\n",
        "\n/remove <code>&lt;account_id&gt;</code>",
        "\n  Remove an email account. Use /list to find the account ID."
    );
    bot.send_message(msg.chat.id, text)
        .parse_mode(ParseMode::Html)
        .await?;
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
        auth_method: "password",
        refresh_token: None,
    };

    let id = db::add_account(pool, &account, chat_id).await?;
    let text = format!(
        "Account added (id: {id}).\n<code>{}</code> → {}:{imap_port}",
        escape_html(label),
        escape_html(&imap_host),
    );
    bot.send_message(msg.chat.id, text)
        .parse_mode(ParseMode::Html)
        .await?;
    tracing::info!("Account added via bot: id={id}, label={label}, chat_id={chat_id}");
    Ok(())
}

async fn handle_add_oauth(
    bot: &Bot,
    msg: &Message,
    oauth_config: &OAuthConfig,
    label: &str,
    provider_name: &str,
    username: &str,
) -> Result<()> {
    let chat_id = msg.chat.id.0;

    let p = provider::lookup(provider_name).ok_or_else(|| {
        anyhow::anyhow!("Unknown provider. Use /providers to see available options.")
    })?;

    let oauth_provider = oauth::OAuthProvider::from_imap_host(p.imap_host());
    let oauth_provider = match oauth_provider {
        Some(op) => op,
        None => {
            bot.send_message(
                msg.chat.id,
                "This provider does not support OAuth. Use /add instead.",
            )
            .await?;
            return Ok(());
        }
    };

    let credentials = oauth::get_credentials(oauth_config, oauth_provider)?;
    let redirect_uri = credentials.redirect_uri().ok_or_else(|| {
        anyhow::anyhow!(
            "No redirect_uri configured for {}. Set it in [oauth.{}] config.",
            oauth_provider.name(),
            provider_name.to_ascii_lowercase(),
        )
    })?;

    let state = oauth::OAuthState {
        label: label.to_owned(),
        provider_name: provider_name.to_owned(),
        username: username.to_owned(),
        chat_id,
    };
    let state_encoded = state.encode();

    let auth_url = oauth::build_authorization_url(
        oauth_provider,
        credentials,
        redirect_uri,
        &state_encoded,
        username,
    )?;

    let text = format!(
        "Click the link below to authorize:\n\n\
         <a href=\"{auth_url}\">Authorize {}</a>\n\n\
         After granting access, the account will be added automatically.",
        escape_html(oauth_provider.name()),
    );
    bot.send_message(msg.chat.id, text)
        .parse_mode(ParseMode::Html)
        .link_preview_options(LinkPreviewOptions {
            is_disabled: true,
            url: None,
            prefer_small_media: false,
            prefer_large_media: false,
            show_above_text: false,
        })
        .await?;
    tracing::info!("OAuth auth URL sent: provider={provider_name}, chat_id={chat_id}");
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
    format_user_info(&mut msg, chat_id, username, first_name, last_name);
    msg
}

fn format_auth_request(
    chat_id: i64,
    username: Option<&str>,
    first_name: Option<&str>,
    last_name: Option<&str>,
) -> String {
    let mut msg = String::from("🔐 <b>Authorization request</b>\n\n");
    format_user_info(&mut msg, chat_id, username, first_name, last_name);
    msg
}

fn format_user_info(
    msg: &mut String,
    chat_id: i64,
    username: Option<&str>,
    first_name: Option<&str>,
    last_name: Option<&str>,
) {
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
}

async fn handle_callback_query(
    bot: Bot,
    q: CallbackQuery,
    pool: SqlitePool,
    admin_chat_id: i64,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let caller_chat_id = q.from.id.0 as i64;
    let data = q.data.as_deref().unwrap_or("");

    // Show more / show less — available to any user
    if let Some(id_str) = data.strip_prefix("show:") {
        let body_id: i64 = id_str.parse()?;
        handle_show_more(&bot, &q, &pool, body_id).await?;
        bot.answer_callback_query(q.id.clone()).await?;
        return Ok(());
    } else if let Some(id_str) = data.strip_prefix("hide:") {
        let body_id: i64 = id_str.parse()?;
        handle_show_less(&bot, &q, &pool, body_id).await?;
        bot.answer_callback_query(q.id.clone()).await?;
        return Ok(());
    }

    // Admin-only callbacks below
    if caller_chat_id != admin_chat_id {
        bot.answer_callback_query(q.id.clone())
            .text("Not authorized.")
            .await?;
        return Ok(());
    }

    if let Some(target_str) = data.strip_prefix("allow:") {
        let target_chat_id: i64 = target_str.parse()?;
        let allowed = db::allow_user(&pool, target_chat_id).await?;

        if let Some(msg) = q.regular_message() {
            let original = msg.text().unwrap_or("");
            let status = if allowed {
                "✅ Allowed"
            } else {
                "⚠️ User not found (hasn't /start-ed)"
            };
            bot.edit_message_text(
                msg.chat.id,
                msg.id,
                format!("{original}\n\n<b>Status:</b> {status}"),
            )
            .parse_mode(ParseMode::Html)
            .await
            .ok();
        }

        if allowed {
            send_notification(
                &bot,
                target_chat_id,
                "✅ You have been authorized! You can now use bot commands.",
            )
            .await
            .ok();
        }

        bot.answer_callback_query(q.id.clone()).await?;
    } else if let Some(target_str) = data.strip_prefix("deny:") {
        let target_chat_id: i64 = target_str.parse()?;

        if let Some(msg) = q.regular_message() {
            let original = msg.text().unwrap_or("");
            bot.edit_message_text(
                msg.chat.id,
                msg.id,
                format!("{original}\n\n<b>Status:</b> ❌ Denied"),
            )
            .parse_mode(ParseMode::Html)
            .await
            .ok();
        }

        send_notification(&bot, target_chat_id, "Your access request was denied.")
            .await
            .ok();

        db::reset_admin_notified(&pool, target_chat_id).await?;

        bot.answer_callback_query(q.id.clone()).await?;
    }

    Ok(())
}

async fn handle_show_more(
    bot: &Bot,
    q: &CallbackQuery,
    pool: &SqlitePool,
    body_id: i64,
) -> anyhow::Result<()> {
    let Some(email_body) = db::get_email_body(pool, body_id).await? else {
        bot.answer_callback_query(q.id.clone())
            .text("Email body no longer available.")
            .await?;
        return Ok(());
    };

    let expanded = assemble_message(
        email_body.header_html(),
        email_body.full_body(),
        email_body.attachments_html(),
    );

    let keyboard = InlineKeyboardMarkup::new(vec![vec![InlineKeyboardButton::callback(
        "Show less",
        format!("hide:{body_id}"),
    )]]);

    if let Some(msg) = q.regular_message() {
        bot.edit_message_text(msg.chat.id, msg.id, expanded)
            .parse_mode(ParseMode::Html)
            .reply_markup(keyboard)
            .await
            .ok();
    }

    Ok(())
}

async fn handle_show_less(
    bot: &Bot,
    q: &CallbackQuery,
    pool: &SqlitePool,
    body_id: i64,
) -> anyhow::Result<()> {
    let Some(email_body) = db::get_email_body(pool, body_id).await? else {
        bot.answer_callback_query(q.id.clone())
            .text("Email body no longer available.")
            .await?;
        return Ok(());
    };

    let preview = make_preview(email_body.full_body());
    let collapsed = assemble_message(
        email_body.header_html(),
        &preview,
        email_body.attachments_html(),
    );

    let keyboard = InlineKeyboardMarkup::new(vec![vec![InlineKeyboardButton::callback(
        "Show more",
        format!("show:{body_id}"),
    )]]);

    if let Some(msg) = q.regular_message() {
        bot.edit_message_text(msg.chat.id, msg.id, collapsed)
            .parse_mode(ParseMode::Html)
            .reply_markup(keyboard)
            .await
            .ok();
    }

    Ok(())
}
