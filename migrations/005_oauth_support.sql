-- Add OAuth2 support columns to email_accounts.
-- auth_method: "password" (default, backward compat) or "xoauth2"
ALTER TABLE email_accounts ADD COLUMN auth_method TEXT NOT NULL DEFAULT 'password';
ALTER TABLE email_accounts ADD COLUMN refresh_token TEXT;
ALTER TABLE email_accounts ADD COLUMN access_token TEXT;
