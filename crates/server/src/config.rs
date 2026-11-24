use std::net::SocketAddr;
use std::path::PathBuf;

use crate::error::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CookieSecure {
    Auto,
    Yes,
    No,
}

#[derive(Debug, Clone)]
pub struct Config {
    pub data_dir: PathBuf,
    pub listen: SocketAddr,
    pub shell_dir: PathBuf,
    pub public_url: Option<String>,
    pub admin_user: String,
    pub admin_password: Option<String>,
    pub cookie_secure: CookieSecure,
    pub session_ttl_secs: i64,
    pub skip_app_sync: bool,
    pub max_upload_bytes: u64,
    pub argon_memory_kib: u32,
    pub argon_time: u32,
    pub argon_parallelism: u32,
    pub oidc_issuer: Option<String>,
    /// Mail server base URL to suggest in Settings › Mail (`ARTCRAFT_MAIL_URL`).
    pub mail_url: Option<String>,
    /// Where the app registry comes from and the keys it must be signed with.
    pub registry: crate::registry::RegistryConfig,
    /// What a registry block does to an installed version (`ARTCRAFT_BLOCKED_APPS`).
    pub blocked_apps: crate::apps::BlockAction,
}

impl Config {
    pub fn from_env() -> Result<Self, Error> {
        let data_dir = env_path("ARTCRAFT_DATA").or_else(|| env_path("ARTCRAFT_DATA_DIR")).unwrap_or_else(|| PathBuf::from("data"));
        let listen = std::env::var("ARTCRAFT_LISTEN")
            .unwrap_or_else(|_| "0.0.0.0:8080".into())
            .parse()
            .map_err(|_| Error::Invalid("ARTCRAFT_LISTEN is not a socket address".into()))?;
        let shell_dir = env_path("ARTCRAFT_SHELL_DIR").unwrap_or_else(|| PathBuf::from("crates/shell/dist"));
        let public_url = std::env::var("ARTCRAFT_PUBLIC_URL").ok().filter(|s| !s.is_empty());
        let admin_user = std::env::var("ARTCRAFT_ADMIN_USER").unwrap_or_else(|_| "admin".into());
        let admin_password = std::env::var("ARTCRAFT_ADMIN_PASSWORD").ok().filter(|s| !s.is_empty());
        let cookie_secure = match std::env::var("ARTCRAFT_COOKIE_SECURE").ok().as_deref() {
            Some("1") | Some("true") | Some("yes") => CookieSecure::Yes,
            Some("0") | Some("false") | Some("no") => CookieSecure::No,
            _ => CookieSecure::Auto,
        };
        let session_ttl_secs = std::env::var("ARTCRAFT_SESSION_HOURS")
            .ok()
            .and_then(|h| h.parse::<i64>().ok())
            .filter(|h| *h > 0)
            .map(|h| h.saturating_mul(3600))
            .unwrap_or(14 * 24 * 3600);
        let skip_app_sync = matches!(std::env::var("ARTCRAFT_SKIP_APP_SYNC").ok().as_deref(), Some("1") | Some("true") | Some("yes"));
        let max_upload_bytes = std::env::var("ARTCRAFT_MAX_UPLOAD_MB")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .filter(|v| *v > 0)
            .map(|mb| mb.saturating_mul(1024 * 1024))
            .unwrap_or(512 * 1024 * 1024);
        let oidc_issuer = std::env::var("ARTCRAFT_OIDC_ISSUER").ok().filter(|s| !s.is_empty());
        let mail_url = std::env::var("ARTCRAFT_MAIL_URL").ok().filter(|s| !s.is_empty());
        Ok(Self {
            data_dir,
            listen,
            shell_dir,
            public_url,
            admin_user,
            admin_password,
            cookie_secure,
            session_ttl_secs,
            skip_app_sync,
            max_upload_bytes,
            argon_memory_kib: 19 * 1024,
            argon_time: 2,
            argon_parallelism: 1,
            oidc_issuer,
            mail_url,
            registry: crate::registry::RegistryConfig::from_env(),
            blocked_apps: crate::apps::BlockAction::from_env(),
        })
    }

    pub fn for_tests(data: &std::path::Path) -> Self {
        Self {
            data_dir: data.to_path_buf(),
            listen: "127.0.0.1:0".parse().expect("addr"),
            shell_dir: data.join("shell"),
            public_url: None,
            admin_user: "admin".into(),
            admin_password: Some("admin-password-test".into()),
            cookie_secure: CookieSecure::No,
            session_ttl_secs: 3600,
            skip_app_sync: true,
            max_upload_bytes: 8 * 1024 * 1024,
            argon_memory_kib: 8,
            argon_time: 1,
            argon_parallelism: 1,
            oidc_issuer: None,
            mail_url: None,
            registry: crate::registry::RegistryConfig::builtin(),
            blocked_apps: crate::apps::BlockAction::Security,
        }
    }

    pub fn cookie_is_secure(&self) -> bool {
        match self.cookie_secure {
            CookieSecure::Yes => true,
            CookieSecure::No => false,
            CookieSecure::Auto => self.public_url.as_deref().is_some_and(|url| url.starts_with("https://")),
        }
    }
}

fn env_path(key: &str) -> Option<PathBuf> {
    std::env::var(key).ok().filter(|s| !s.is_empty()).map(PathBuf::from)
}
