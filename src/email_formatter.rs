use anyhow::Result;
use mailparse::{MailHeaderMap, ParsedMail};

const PREVIEW_MAX_CHARS: usize = 200;
const TELEGRAM_MAX_LEN: usize = 4096;

pub struct ParsedEmail {
    from: String,
    subject: String,
    date: String,
    body_preview: String,
    body_full: String,
    attachments: Vec<String>,
}

impl ParsedEmail {
    pub fn from(&self) -> &str {
        &self.from
    }

    pub fn subject(&self) -> &str {
        &self.subject
    }

    pub fn date(&self) -> &str {
        &self.date
    }

    pub fn body_preview(&self) -> &str {
        &self.body_preview
    }

    pub fn body_full(&self) -> &str {
        &self.body_full
    }

    pub fn attachments(&self) -> &[String] {
        &self.attachments
    }

    pub fn was_truncated(&self) -> bool {
        self.body_full.len() > self.body_preview.len()
    }
}

pub struct FormattedParts {
    header_html: String,
    attachments_html: String,
}

impl FormattedParts {
    pub fn header_html(&self) -> &str {
        &self.header_html
    }

    pub fn attachments_html(&self) -> &str {
        &self.attachments_html
    }
}

pub fn parse_email(raw: &[u8]) -> Result<ParsedEmail> {
    let mail = mailparse::parse_mail(raw)?;
    let headers = mail.get_headers();

    let from = headers
        .get_first_value("From")
        .unwrap_or_else(|| "(unknown sender)".to_string());

    let subject = headers
        .get_first_value("Subject")
        .unwrap_or_else(|| "(no subject)".to_string());

    let date = headers
        .get_first_value("Date")
        .unwrap_or_else(|| "(no date)".to_string());

    let (body_full, body_preview, attachments) = extract_body_and_attachments(&mail);

    Ok(ParsedEmail {
        from,
        subject,
        date,
        body_preview,
        body_full,
        attachments,
    })
}

fn extract_body_and_attachments(mail: &ParsedMail) -> (String, String, Vec<String>) {
    let mut body = String::new();
    let mut attachments = Vec::new();

    visit_parts(mail, &mut body, &mut attachments, true);

    let truncated = truncate_at_char_boundary(&body, PREVIEW_MAX_CHARS);
    let preview = if truncated.len() < body.len() {
        format!("{truncated}…")
    } else {
        body.clone()
    };

    (body, preview, attachments)
}

fn visit_parts(part: &ParsedMail, body: &mut String, attachments: &mut Vec<String>, is_root: bool) {
    let content_type = part.ctype.mimetype.to_lowercase();
    let disposition = part
        .get_headers()
        .get_first_value("Content-Disposition")
        .unwrap_or_default()
        .to_lowercase();

    let is_attachment = disposition.starts_with("attachment");

    if is_attachment {
        let filename = extract_filename(part);
        let size = part.get_body_raw().map(|b| b.len()).unwrap_or(0);
        attachments.push(format_attachment_entry(&filename, size));
        return;
    }

    if content_type == "text/plain" && body.is_empty() {
        if let Ok(text) = part.get_body() {
            *body = text.trim().to_string();
        }
        return;
    }

    if content_type.starts_with("multipart/") || is_root {
        for sub in &part.subparts {
            visit_parts(sub, body, attachments, false);
        }
    }
}

fn extract_filename(part: &ParsedMail) -> String {
    let disposition = part
        .get_headers()
        .get_first_value("Content-Disposition")
        .unwrap_or_default();

    for param in disposition.split(';') {
        let param = param.trim();
        if let Some(val) = param.strip_prefix("filename=") {
            return val.trim_matches('"').to_string();
        }
    }

    let content_type = part
        .get_headers()
        .get_first_value("Content-Type")
        .unwrap_or_default();

    for param in content_type.split(';') {
        let param = param.trim();
        if let Some(val) = param.strip_prefix("name=") {
            return val.trim_matches('"').to_string();
        }
    }

    "(unnamed)".to_string()
}

fn format_attachment_entry(filename: &str, size_bytes: usize) -> String {
    if size_bytes >= 1024 * 1024 {
        format!(
            "{filename} ({:.1} MB)",
            size_bytes as f64 / (1024.0 * 1024.0)
        )
    } else if size_bytes >= 1024 {
        format!("{filename} ({:.1} KB)", size_bytes as f64 / 1024.0)
    } else {
        format!("{filename} ({size_bytes} B)")
    }
}

/// Build the header and attachments HTML portions of a Telegram message.
pub fn format_message_parts(account_label: &str, email: &ParsedEmail) -> FormattedParts {
    let header_html = format!(
        "📧 <b>New email</b> [{account_label}]\n\
         <b>From:</b> {from}\n\
         <b>Subject:</b> {subject}\n\
         <b>Date:</b> {date}",
        from = escape_html(email.from()),
        subject = escape_html(email.subject()),
        date = escape_html(email.date()),
    );

    let mut attachments_html = String::new();
    if !email.attachments().is_empty() {
        attachments_html.push_str("\n\n<b>Attachments:</b>");
        for att in email.attachments() {
            attachments_html.push_str(&format!("\n• {}", escape_html(att)));
        }
    }

    FormattedParts {
        header_html,
        attachments_html,
    }
}

/// Assemble a complete Telegram message from stored parts and a body string.
/// Truncates the body if the total would exceed Telegram's 4096-char limit.
pub fn assemble_message(header_html: &str, body: &str, attachments_html: &str) -> String {
    // 2 newlines before body + 2 before attachments (worst case)
    let overhead = header_html.len() + attachments_html.len() + 4;
    let body_budget = TELEGRAM_MAX_LEN.saturating_sub(overhead);

    let escaped = escape_html(body);
    let body_part = if escaped.len() > body_budget {
        let truncated = truncate_at_char_boundary(&escaped, body_budget.saturating_sub(4));
        format!("{truncated}…")
    } else {
        escaped
    };

    let mut msg = header_html.to_owned();
    if !body_part.is_empty() {
        msg.push_str("\n\n");
        msg.push_str(&body_part);
    }
    if !attachments_html.is_empty() {
        msg.push_str(attachments_html);
    }
    msg
}

/// Build a preview body string (truncated with ellipsis) from the full body.
pub fn make_preview(full_body: &str) -> String {
    let truncated = truncate_at_char_boundary(full_body, PREVIEW_MAX_CHARS);
    if truncated.len() < full_body.len() {
        format!("{truncated}…")
    } else {
        full_body.to_owned()
    }
}

pub fn format_telegram_message(account_label: &str, email: &ParsedEmail) -> String {
    let parts = format_message_parts(account_label, email);
    assemble_message(
        parts.header_html(),
        email.body_preview(),
        parts.attachments_html(),
    )
}

pub fn escape_html(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// Truncate a string at a char boundary, returning at most `max_chars` characters.
pub fn truncate_at_char_boundary(s: &str, max_chars: usize) -> &str {
    match s.char_indices().nth(max_chars) {
        Some((byte_idx, _)) => &s[..byte_idx],
        None => s,
    }
}
