pub struct Provider {
    imap_host: &'static str,
    imap_port: u16,
}

impl Provider {
    pub fn imap_host(&self) -> &str {
        self.imap_host
    }

    pub fn imap_port(&self) -> u16 {
        self.imap_port
    }
}

const BUILT_IN: &[(&str, Provider)] = &[
    (
        "gmail",
        Provider {
            imap_host: "imap.gmail.com",
            imap_port: 993,
        },
    ),
    (
        "outlook",
        Provider {
            imap_host: "outlook.office365.com",
            imap_port: 993,
        },
    ),
    (
        "yahoo",
        Provider {
            imap_host: "imap.mail.yahoo.com",
            imap_port: 993,
        },
    ),
    (
        "icloud",
        Provider {
            imap_host: "imap.mail.me.com",
            imap_port: 993,
        },
    ),
    (
        "fastmail",
        Provider {
            imap_host: "imap.fastmail.com",
            imap_port: 993,
        },
    ),
];

/// Look up a built-in provider by name (case-insensitive).
pub fn lookup(name: &str) -> Option<&'static Provider> {
    let lower = name.to_ascii_lowercase();
    BUILT_IN.iter().find(|(n, _)| *n == lower).map(|(_, p)| p)
}

/// Return all built-in providers.
pub fn all() -> &'static [(&'static str, Provider)] {
    BUILT_IN
}

/// Look up the built-in provider for an OAuth provider.
pub fn lookup_by_oauth_provider(oauth_provider: crate::oauth::OAuthProvider) -> &'static Provider {
    match oauth_provider {
        crate::oauth::OAuthProvider::Google => lookup("gmail").unwrap(),
        crate::oauth::OAuthProvider::Microsoft => lookup("outlook").unwrap(),
    }
}
