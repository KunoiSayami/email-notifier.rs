# email-notifier

An async Rust daemon that monitors multiple email accounts via IMAP IDLE and forwards new emails to Telegram.

## Features

- **Real-time notifications** — uses IMAP IDLE for instant push; falls back to periodic polling on timeout
- **Multiple accounts** — monitor as many IMAP mailboxes as you like
- **Shared accounts** — multiple Telegram users can subscribe to the same email account (single IMAP connection)
- **OAuth2 support** — XOAUTH2 authentication for Gmail and Outlook (no plain passwords needed)
- **No duplicates** — tracks seen message UIDs in SQLite so restarts don't re-notify
- **Hot-reload config** — watches `config.toml` for changes and restarts monitors automatically
- **IPC control** — manage the running daemon via local socket commands (`ipc reload`, `ipc status`, `ipc list`)
- **Telegram bot management** — add/remove accounts and manage subscriptions via bot commands
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
admin_chat_id = 123456789

[database]
path = "email-notifier.db"

[log]
level = "info"

# Optional: OAuth2 credentials for Gmail / Outlook
[oauth.google]
client_id = "xxx.apps.googleusercontent.com"
client_secret = "GOCSPX-xxx"

[oauth.microsoft]
client_id = "xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx"
client_secret = "xxx"
```

### 2. Add an email account

**Password auth** (works with app passwords or non-OAuth providers):

```bash
email-notifier account add \
  --label personal \
  --provider gmail \
  --username you@gmail.com \
  --password "your-app-password" \
  --chat-id 123456789
```

> For Gmail, use an [App Password](https://support.google.com/accounts/answer/185833) instead of your regular password.

**OAuth2 auth** (Gmail / Outlook — no plain password needed):

```bash
email-notifier account add \
  --label work-gmail \
  --provider gmail \
  --username you@gmail.com \
  --auth oauth \
  --refresh-token "1//0abc..." \
  --chat-id 123456789
```

> You need to obtain a refresh token externally (e.g. via [Google OAuth Playground](https://developers.google.com/oauthplayground/)) and configure `[oauth.google]` in your config file.

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
  ipc reload         Trigger daemon to reload accounts from database
  ipc status         Check if the daemon is running
  ipc list           List all email accounts via daemon

Options:
  -c, --config <PATH>  Path to config file [default: config.toml]
```

### Managing accounts

```bash
# List all accounts
email-notifier account list

# Remove an account
email-notifier account remove 1

# List built-in providers
email-notifier account add --list-providers
```

### IPC (controlling the running daemon)

```bash
# Check daemon status
email-notifier ipc status

# Reload accounts from database (no restart needed)
email-notifier ipc reload

# List accounts via daemon
email-notifier ipc list
```

## Building

```bash
cargo build --release
```

## Telegram Bot Commands

| Command | Description |
|---|---|
| `/start` | Register with the bot |
| `/id` | Show your chat ID |
| `/providers` | List built-in email providers |
| `/add label provider username password` | Add a password-auth account |
| `/addoauth label provider username refresh_token` | Add an OAuth2 account |
| `/list` | List your subscribed accounts |
| `/remove id` | Unsubscribe from an account |
| `/allow chat_id` | (Admin) Grant access to a user |

## How It Works

1. On startup, the daemon loads the config, initializes SQLite, and spawns one async task per unique email account.
2. Each task connects to the IMAP server over TLS (using password login or XOAUTH2), selects INBOX, and marks all existing UIDs as seen.
3. The task enters IMAP IDLE and waits for the server to signal new mail.
4. On wakeup, it searches for UNSEEN messages, fetches each one, parses headers and body, formats an HTML message, and sends it to **all subscribers** via the Telegram Bot API.
5. Seen UIDs are recorded in SQLite to prevent duplicates across restarts.
6. If the connection drops, the task reconnects with exponential backoff.
7. For OAuth2 accounts, access tokens are automatically refreshed before each connection attempt.

## Project Structure

```
src/
├── main.rs              Entry point, CLI dispatch, daemon loop
├── config.rs            TOML config loading + file-watch reload
├── db.rs                SQLite setup, account CRUD, UID tracking
├── imap_monitor.rs      Per-account IMAP IDLE loop with reconnect
├── ipc.rs               IPC server (local socket, JSON protocol)
├── oauth.rs             OAuth2 token refresh + XOAUTH2 authenticator
├── provider.rs          Built-in email provider definitions
├── telegram.rs          Bot creation + send_notification helper
├── bot_commands.rs      Telegram bot command handler
├── email_formatter.rs   Email parsing + Telegram message formatting
└── cli/
    ├── mod.rs           Clap CLI structs
    ├── account_add.rs   Add account handler
    ├── account_list.rs  List accounts handler
    ├── account_remove.rs Remove account handler
    └── ipc_client.rs    IPC client (connects to running daemon)
migrations/
├── 001_init.sql         Initial schema (seen_uids, bot_users)
├── 004_dedup_accounts.sql  Deduplicated email_accounts + subscriptions
└── 005_oauth_support.sql   OAuth2 columns (auth_method, tokens)
```

## License

[![](https://www.gnu.org/graphics/agplv3-155x51.png "AGPL v3 logo")](https://www.gnu.org/licenses/agpl-3.0.txt)

Copyright (C) 2026 KunoiSayami

This program is free software: you can redistribute it and/or modify it under the terms of the GNU Affero General Public License as published by the Free Software Foundation, either version 3 of the License, or any later version.

This program is distributed in the hope that it will be useful, but WITHOUT ANY WARRANTY; without even the implied warranty of MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE. See the GNU Affero General Public License for more details.

You should have received a copy of the GNU Affero General Public License along with this program. If not, see <https://www.gnu.org/licenses/>.
