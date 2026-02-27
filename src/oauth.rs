use anyhow::{Context, Result, bail};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use serde::{Deserialize, Serialize};

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

    pub fn authorization_endpoint(&self) -> &'static str {
        match self {
            Self::Google => "https://accounts.google.com/o/oauth2/v2/auth",
            Self::Microsoft => "https://login.microsoftonline.com/common/oauth2/v2.0/authorize",
        }
    }

    pub fn scopes(&self) -> &'static str {
        match self {
            Self::Google => "https://mail.google.com/",
            Self::Microsoft => "offline_access https://outlook.office.com/IMAP.AccessAsUser.All",
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            Self::Google => "Google",
            Self::Microsoft => "Microsoft",
        }
    }

    /// Parse provider from user-facing name (case-insensitive).
    pub fn from_name(name: &str) -> Option<Self> {
        match name.to_ascii_lowercase().as_str() {
            "google" | "gmail" => Some(Self::Google),
            "microsoft" | "outlook" => Some(Self::Microsoft),
            _ => None,
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

/// Session data encoded in the OAuth `state` parameter.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OAuthState {
    pub label: String,
    pub provider_name: String,
    pub username: String,
    pub chat_id: i64,
}

impl OAuthState {
    /// Encode as base64url JSON.
    pub fn encode(&self) -> String {
        let json = serde_json::to_string(self).expect("OAuthState serialization cannot fail");
        URL_SAFE_NO_PAD.encode(json.as_bytes())
    }

    /// Decode from base64url JSON.
    pub fn decode(s: &str) -> Result<Self> {
        let bytes = URL_SAFE_NO_PAD
            .decode(s)
            .context("Invalid base64 in OAuth state")?;
        serde_json::from_slice(&bytes).context("Invalid JSON in OAuth state")
    }
}

/// Build an OAuth authorization URL for the user to visit.
pub fn build_authorization_url(
    provider: OAuthProvider,
    credentials: &OAuthClientCredentials,
    redirect_uri: &str,
    state: &str,
    login_hint: &str,
) -> Result<String> {
    let mut url = reqwest::Url::parse(provider.authorization_endpoint())
        .context("Invalid authorization endpoint")?;

    url.query_pairs_mut()
        .append_pair("client_id", credentials.client_id())
        .append_pair("redirect_uri", redirect_uri)
        .append_pair("response_type", "code")
        .append_pair("scope", provider.scopes())
        .append_pair("state", state)
        .append_pair("access_type", "offline")
        .append_pair("prompt", "consent")
        .append_pair("login_hint", login_hint);

    Ok(url.to_string())
}

#[derive(Debug, Deserialize)]
struct CodeExchangeResponse {
    access_token: String,
    refresh_token: Option<String>,
}

/// Exchange an authorization code for access + refresh tokens.
pub async fn exchange_authorization_code(
    provider: OAuthProvider,
    credentials: &OAuthClientCredentials,
    redirect_uri: &str,
    code: &str,
) -> Result<(String, String)> {
    let client = reqwest::Client::new();
    let response = client
        .post(provider.token_endpoint())
        .form(&[
            ("grant_type", "authorization_code"),
            ("client_id", credentials.client_id()),
            ("client_secret", credentials.client_secret()),
            ("code", code),
            ("redirect_uri", redirect_uri),
        ])
        .send()
        .await
        .context("Code exchange HTTP request failed")?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        bail!("Code exchange failed (HTTP {status}): {body}");
    }

    let resp: CodeExchangeResponse = response
        .json()
        .await
        .context("Failed to parse code exchange response")?;

    let refresh_token = resp
        .refresh_token
        .context("No refresh_token in response. Ensure prompt=consent and access_type=offline.")?;

    Ok((resp.access_token, refresh_token))
}

/// XOAUTH2 SASL authenticator for async-imap.
///
/// On the first challenge (empty), we send the XOAUTH2 token string.
/// On any subsequent challenge (error details), we send an empty response
/// to let the server return the actual error instead of hanging.
pub struct XOAuth2 {
    user: String,
    access_token: String,
    sent: bool,
}

impl XOAuth2 {
    pub fn new(user: String, access_token: String) -> Self {
        Self {
            user,
            access_token,
            sent: false,
        }
    }
}

impl async_imap::Authenticator for &mut XOAuth2 {
    type Response = String;

    fn process(&mut self, _challenge: &[u8]) -> Self::Response {
        if self.sent {
            // Server sent an error challenge; respond empty to complete the exchange.
            String::new()
        } else {
            self.sent = true;
            format!(
                "user={}\x01auth=Bearer {}\x01\x01",
                self.user, self.access_token
            )
        }
    }
}
