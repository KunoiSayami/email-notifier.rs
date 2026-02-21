use anyhow::Result;
use teloxide::{
    prelude::*,
    types::{ChatId, ParseMode},
};

pub fn create_bot(token: &str) -> Bot {
    Bot::new(token)
}

pub async fn send_notification(bot: &Bot, chat_id: i64, text: &str) -> Result<()> {
    bot.send_message(ChatId(chat_id), text)
        .parse_mode(ParseMode::Html)
        .await?;
    Ok(())
}
