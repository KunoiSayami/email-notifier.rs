use anyhow::{Context, Result};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

use interprocess::local_socket::{
    GenericNamespaced, ToNsName as _, tokio::Stream, traits::tokio::Stream as _,
};

use crate::cli::IpcAction;

pub async fn run(socket_name: &str, action: &IpcAction) -> Result<()> {
    let name = socket_name
        .to_ns_name::<GenericNamespaced>()
        .context("Invalid socket name")?;

    let conn = Stream::connect(name)
        .await
        .context("Failed to connect to daemon. Is it running?")?;

    let (reader, mut writer) = conn.split();

    let request = match action {
        IpcAction::Reload => r#"{"cmd":"reload"}"#.to_string(),
        IpcAction::Status => r#"{"cmd":"status"}"#.to_string(),
        IpcAction::List => r#"{"cmd":"list_accounts"}"#.to_string(),
    };

    writer
        .write_all(format!("{request}\n").as_bytes())
        .await
        .context("Failed to send request")?;

    let mut lines = BufReader::new(reader).lines();
    if let Some(line) = lines.next_line().await.context("Failed to read response")? {
        let value: serde_json::Value =
            serde_json::from_str(&line).context("Invalid response from daemon")?;
        println!("{}", serde_json::to_string_pretty(&value)?);
    }

    Ok(())
}
