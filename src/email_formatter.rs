use anyhow::Result;
use mailparse::{MailHeaderMap, ParsedMail};

pub struct ParsedEmail {
    from: String,
    subject: String,
    date: String,
    body_preview: String,
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

    pub fn attachments(&self) -> &[String] {
        &self.attachments
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

    let (body_preview, attachments) = extract_body_and_attachments(&mail);

    Ok(ParsedEmail {
        from,
        subject,
        date,
        body_preview,
        attachments,
    })
}

fn extract_body_and_attachments(mail: &ParsedMail) -> (String, Vec<String>) {
    let mut body = String::new();
    let mut attachments = Vec::new();

    visit_parts(mail, &mut body, &mut attachments, true);

    let preview = if body.len() > 500 {
        format!("{}…", &body[..500])
    } else {
        body
    };

    (preview, attachments)
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

pub fn format_telegram_message(account_label: &str, email: &ParsedEmail) -> String {
    let mut msg = format!(
        "📧 <b>New email</b> [{account_label}]\n\
         <b>From:</b> {from}\n\
         <b>Subject:</b> {subject}\n\
         <b>Date:</b> {date}",
        from = escape_html(email.from()),
        subject = escape_html(email.subject()),
        date = escape_html(email.date()),
    );

    if !email.body_preview().is_empty() {
        msg.push_str("\n\n");
        msg.push_str(&escape_html(email.body_preview()));
    }

    if !email.attachments().is_empty() {
        msg.push_str("\n\n<b>Attachments:</b>");
        for att in email.attachments() {
            msg.push_str(&format!("\n• {}", escape_html(att)));
        }
    }

    msg
}

fn escape_html(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}
