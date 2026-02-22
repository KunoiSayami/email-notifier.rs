# email-notifier

An async Rust daemon that monitors multiple email accounts via IMAP IDLE and forwards new emails to Telegram.

## Features

- **Real-time notifications** — uses IMAP IDLE for instant push; falls back to periodic polling on timeout
- **Multiple accounts** — monitor as many IMAP mailboxes as you like, each with its own Telegram chat
- **No duplicates** — tracks seen message UIDs in SQLite so restarts don't re-notify
- **Hot-reload config** — watches `config.toml` for changes and restarts monitors automatically
- **Resilient** — exponential backoff reconnection on IMAP errors (5 s → 5 min)

## Quick Start

### 1. Create a config file

```bash
cp config.toml.example config.toml
```

Edit `config.toml` with your Telegram bot token (get one from [@BotFather](https://t.me/BotFather)):

```toml
[telegram]
bot_token = "123456789:ABC-YourBotTokenHere"

[database]
path = "email-notifier.db"

[log]
level = "info"
```

### 2. Add an email account

```bash
email-notifier account add \
  --label personal \
  --host imap.gmail.com \
  --port 993 \
  --username you@gmail.com \
  --password "your-app-password" \
  --chat-id 123456789
```

> For Gmail, use an [App Password](https://support.google.com/accounts/answer/185833) instead of your regular password.

### 3. Start the daemon

```bash
email-notifier run
```

New emails will appear in your Telegram chat within seconds.

## CLI Reference

```
email-notifier [OPTIONS] <COMMAND>

Commands:
  run                Start the notification daemon
  account add        Add a new IMAP account
  account list       List all configured accounts
  account remove     Remove an account by ID

Options:
  -c, --config <PATH>  Path to config file [default: config.toml]
```

### Managing accounts

```bash
# List all accounts
email-notifier account list

# Remove an account
email-notifier account remove 1
```

## Building

```bash
cargo build --release
```

## How It Works

1. On startup, the daemon loads the config, initializes SQLite, and spawns one async task per account.
2. Each task connects to the IMAP server over TLS, selects INBOX, and marks all existing UIDs as seen.
3. The task enters IMAP IDLE and waits for the server to signal new mail.
4. On wakeup, it searches for UNSEEN messages, fetches each one, parses headers and body, formats an HTML message, and sends it via the Telegram Bot API.
5. Seen UIDs are recorded in SQLite to prevent duplicates across restarts.
6. If the connection drops, the task reconnects with exponential backoff.

## Project Structure

```
src/
├── main.rs              Entry point, CLI dispatch, daemon loop
├── config.rs            TOML config loading + file-watch reload
├── db.rs                SQLite setup, account CRUD, UID tracking
├── imap_monitor.rs      Per-account IMAP IDLE loop with reconnect
├── telegram.rs          Bot creation + send_notification helper
├── email_formatter.rs   Email parsing + Telegram message formatting
└── cli/
    ├── mod.rs           Clap CLI structs
    ├── account_add.rs   Add account handler
    ├── account_list.rs  List accounts handler
    └── account_remove.rs Remove account handler
migrations/
└── 001_init.sql         SQLite schema (accounts + seen_uids)
```

## License

[![](https://www.gnu.org/graphics/agplv3-155x51.png "AGPL v3 logo")](https://www.gnu.org/licenses/agpl-3.0.txt)

Copyright (C) 2026 KunoiSayami

This program is free software: you can redistribute it and/or modify it under the terms of the GNU Affero General Public License as published by the Free Software Foundation, either version 3 of the License, or any later version.

This program is distributed in the hope that it will be useful, but WITHOUT ANY WARRANTY; without even the implied warranty of MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE. See the GNU Affero General Public License for more details.

You should have received a copy of the GNU Affero General Public License along with this program. If not, see <https://www.gnu.org/licenses/>.
