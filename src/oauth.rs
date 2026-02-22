use anyhow::{Context, Result, bail};
use serde::Deserialize;

use crate::config::{OAuthClientCredentials, OAuthConfig};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OAuthProvider {
    Google,
    Microsoft,
}

impl OAuthProvider {
    /// Determine the OAuth provider from an IMAP host.
    pub fn from_imap_host(host: &str) -> Option<Self> {
        match host {
            "imap.gmail.com" => Some(Self::Google),
            "outlook.office365.com" => Some(Self::Microsoft),
            _ => None,
        }
    }

    fn token_endpoint(&self) -> &'static str {
        match self {
            Self::Google => "https://oauth2.googleapis.com/token",
            Self::Microsoft => "https://login.microsoftonline.com/common/oauth2/v2.0/token",
        }
    }
}

/// Get the OAuth client credentials for a given provider from the config.
pub fn get_credentials<'a>(
    oauth_config: &'a OAuthConfig,
    provider: OAuthProvider,
) -> Result<&'a OAuthClientCredentials> {
    match provider {
        OAuthProvider::Google => oauth_config
            .google
            .as_ref()
            .context("Missing [oauth.google] in config"),
        OAuthProvider::Microsoft => oauth_config
            .microsoft
            .as_ref()
            .context("Missing [oauth.microsoft] in config"),
    }
}

#[derive(Debug, Deserialize)]
struct TokenResponse {
    access_token: String,
}

/// Refresh an OAuth2 access token using the provider's token endpoint.
pub async fn refresh_access_token(
    provider: OAuthProvider,
    credentials: &OAuthClientCredentials,
    refresh_token: &str,
) -> Result<String> {
    let client = reqwest::Client::new();
    let response = client
        .post(provider.token_endpoint())
        .form(&[
            ("grant_type", "refresh_token"),
            ("client_id", credentials.client_id()),
            ("client_secret", credentials.client_secret()),
            ("refresh_token", refresh_token),
        ])
        .send()
        .await
        .context("Token refresh HTTP request failed")?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        bail!("Token refresh failed (HTTP {status}): {body}");
    }

    let token_resp: TokenResponse = response
        .json()
        .await
        .context("Failed to parse token refresh response")?;

    Ok(token_resp.access_token)
}

/// XOAUTH2 SASL authenticator for async-imap.
pub struct XOAuth2 {
    user: String,
    access_token: String,
}

impl XOAuth2 {
    pub fn new(user: String, access_token: String) -> Self {
        Self { user, access_token }
    }
}

impl async_imap::Authenticator for &XOAuth2 {
    type Response = String;

    fn process(&mut self, _challenge: &[u8]) -> Self::Response {
        format!(
            "user={}\x01auth=Bearer {}\x01\x01",
            self.user, self.access_token
        )
    }
}
