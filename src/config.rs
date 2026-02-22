use anyhow::{Context, Result};
use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use serde::Deserialize;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use tokio::sync::watch;

#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    pub telegram: TelegramConfig,
    pub database: DatabaseConfig,
    #[serde(default)]
    pub log: LogConfig,
    #[serde(default)]
    pub ipc: IpcConfig,
    #[serde(default)]
    pub oauth: OAuthConfig,
}

impl Config {
    pub fn log(&self) -> &LogConfig {
        &self.log
    }

    pub fn ipc(&self) -> &IpcConfig {
        &self.ipc
    }

    pub fn oauth(&self) -> &OAuthConfig {
        &self.oauth
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct OAuthConfig {
    pub google: Option<OAuthClientCredentials>,
    pub microsoft: Option<OAuthClientCredentials>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct OAuthClientCredentials {
    client_id: String,
    client_secret: String,
}

impl OAuthClientCredentials {
    pub fn client_id(&self) -> &str {
        &self.client_id
    }

    pub fn client_secret(&self) -> &str {
        &self.client_secret
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct IpcConfig {
    #[serde(default = "default_socket_name")]
    socket_name: String,
}

impl IpcConfig {
    pub fn socket_name(&self) -> &str {
        &self.socket_name
    }
}

fn default_socket_name() -> String {
    "email-notifier.sock".to_string()
}

impl Default for IpcConfig {
    fn default() -> Self {
        Self {
            socket_name: default_socket_name(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct TelegramConfig {
    pub bot_token: String,
    pub admin_chat_id: i64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DatabaseConfig {
    #[serde(default = "default_db_path")]
    pub path: String,
}

fn default_db_path() -> String {
    "email-notifier.db".to_string()
}

#[derive(Debug, Clone, Deserialize)]
pub struct LogConfig {
    #[serde(default = "default_log_level")]
    pub level: String,
    #[serde(default = "default_modifier")]
    modifier: String,
}

impl LogConfig {
    pub fn level(&self) -> String {
        if !self.level.contains(",") {
            format!("{},{}", self.level, self.modifier)
        } else {
            self.level.clone()
        }
    }
}

fn default_modifier() -> String {
    "hyper_util=warn,sqlx=warn,teloxide=warn,reqwest=warn".to_string()
}
fn default_log_level() -> String {
    "info".to_string()
}

impl Default for LogConfig {
    fn default() -> Self {
        Self {
            level: default_log_level(),
            modifier: default_modifier(),
        }
    }
}

pub fn load_config(path: &Path) -> Result<Config> {
    let contents = std::fs::read_to_string(path)
        .with_context(|| format!("Failed to read config file: {}", path.display()))?;
    toml::from_str(&contents).with_context(|| "Failed to parse config file")
}

/// Spawns a background thread that watches `path` for modifications.
/// Sends the newly loaded Config through the returned watch::Receiver.
pub fn watch_config(path: PathBuf) -> Result<watch::Receiver<Config>> {
    let initial = load_config(&path)?;
    let (tx, rx) = watch::channel(initial);

    std::thread::spawn(move || {
        let (fs_tx, fs_rx) = mpsc::channel::<notify::Result<Event>>();

        let mut watcher = match RecommendedWatcher::new(fs_tx, notify::Config::default()) {
            Ok(w) => w,
            Err(e) => {
                tracing::error!("Failed to create file watcher: {e}");
                return;
            }
        };

        if let Err(e) = watcher.watch(&path, RecursiveMode::NonRecursive) {
            tracing::error!("Failed to watch config file: {e}");
            return;
        }

        for result in fs_rx {
            match result {
                Ok(event) if matches!(event.kind, EventKind::Modify(_)) => {
                    match load_config(&path) {
                        Ok(config) => {
                            tracing::info!("Config reloaded from {}", path.display());
                            let _ = tx.send(config);
                        }
                        Err(e) => tracing::warn!("Failed to reload config: {e}"),
                    }
                }
                Err(e) => tracing::warn!("File watch error: {e}"),
                _ => {}
            }
        }
    });

    Ok(rx)
}
