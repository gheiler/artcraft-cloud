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
    /// Origin isolated (sideloaded) apps are served from (`ARTCRAFT_APPS_URL`).
    pub apps_url: Option<String>,
}

impl Config {
    pub fn from_env() -> Result<Self, Error> {
        let data_dir = env_path("ARTCRAFT_DATA").or_else(|| env_path("ARTCRAFT_DATA_DIR")).unwrap_or_else(|| PathBuf::from("data"));
        let listen = std::env::var("ARTCRAFT_LISTEN")
            .unwrap_or_else(|_| "0.0.0.0:8080".into())
            .parse::<SocketAddr>()
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
        let apps_url = parse_apps_url(std::env::var("ARTCRAFT_APPS_URL").ok(), public_url.as_deref(), listen.port());
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
            apps_url,
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
            // Same process, second Host header: browsers resolve *.localhost to the loopback.
            apps_url: Some("http://apps.localhost".into()),
        }
    }

    pub fn cookie_is_secure(&self) -> bool {
        match self.cookie_secure {
            CookieSecure::Yes => true,
            CookieSecure::No => false,
            CookieSecure::Auto => self.public_url.as_deref().is_some_and(|url| url.starts_with("https://")),
        }
    }

    /// Origin isolated apps are served from, with no trailing slash.
    pub fn apps_origin(&self) -> Option<&str> {
        self.apps_url.as_deref()
    }

    /// The shell's public origin, when `ARTCRAFT_PUBLIC_URL` is set.
    pub fn public_origin(&self) -> Option<String> {
        self.public_url.as_deref().and_then(|url| origin_of(url).ok())
    }

    pub fn is_apps_host(&self, host: &str) -> bool {
        let Some(apps) = self.apps_url.as_deref() else {
            return false;
        };
        host_matches(apps, host)
    }
}

/// `ARTCRAFT_APPS_URL` if set (empty disables isolation). Otherwise a loopback default when the
/// public URL is local or unset, so Docker Compose and `cargo run` work without extra DNS.
fn parse_apps_url(explicit: Option<String>, public_url: Option<&str>, listen_port: u16) -> Option<String> {
    match explicit {
        Some(value) => {
            let value = value.trim();
            if value.is_empty() || value == "off" || value == "0" {
                return None;
            }
            origin_of(value).ok()
        }
        None => default_apps_url(public_url, listen_port),
    }
}

fn default_apps_url(public_url: Option<&str>, listen_port: u16) -> Option<String> {
    let Some(public) = public_url.filter(|s| !s.is_empty()) else {
        return Some(format!("http://apps.localhost:{listen_port}"));
    };
    let origin = origin_of(public).ok()?;
    let host = host_of(&origin)?;
    let (name, port) = split_host_port(&host);
    if is_loopback_name(name) {
        let port = port.unwrap_or(if origin.starts_with("https://") { 443 } else { 80 });
        let default_port = origin.starts_with("https://") && port == 443 || origin.starts_with("http://") && port == 80;
        return Some(if default_port {
            format!("{}://apps.localhost", if origin.starts_with("https://") { "https" } else { "http" })
        } else {
            format!("{}://apps.localhost:{port}", if origin.starts_with("https://") { "https" } else { "http" })
        });
    }
    // Production: do not guess an `apps.` hostname (it needs DNS and a certificate).
    None
}

/// Scheme + host[+port], no path, or an error if the URL is not a plain http(s) origin.
pub fn origin_of(url: &str) -> Result<String, String> {
    let url = url.trim().trim_end_matches('/');
    let (scheme, rest) = url.split_once("://").ok_or_else(|| "apps origin needs an http(s) URL".to_string())?;
    if scheme != "http" && scheme != "https" {
        return Err("apps origin must be http or https".into());
    }
    let host = rest.split('/').next().unwrap_or("").split('?').next().unwrap_or("").trim();
    if host.is_empty() || host.contains(' ') || host.contains(char::from(92)) {
        return Err("apps origin has no host".into());
    }
    Ok(format!("{scheme}://{host}"))
}

fn host_of(origin: &str) -> Option<String> {
    origin.split_once("://").map(|(_, rest)| rest.to_ascii_lowercase())
}

fn split_host_port(host: &str) -> (&str, Option<u16>) {
    if let Some(rest) = host.strip_prefix('[') {
        if let Some((name, port)) = rest.split_once("]:") {
            return (name, port.parse().ok());
        }
        return (rest.trim_end_matches(']'), None);
    }
    match host.rsplit_once(':') {
        Some((name, port)) if !name.is_empty() && port.chars().all(|c| c.is_ascii_digit()) => (name, port.parse().ok()),
        _ => (host, None),
    }
}

fn is_loopback_name(name: &str) -> bool {
    matches!(name, "localhost" | "127.0.0.1" | "::1" | "[::1]") || name.ends_with(".localhost")
}

/// Compare a configured origin to a request `Host` header (port included when not default).
pub fn host_matches(origin: &str, host_header: &str) -> bool {
    let Ok(origin) = origin_of(origin) else {
        return false;
    };
    let Some(want) = host_of(&origin) else {
        return false;
    };
    let got = host_header.trim().trim_end_matches('.').to_ascii_lowercase();
    if want == got {
        return true;
    }
    // `http://apps.localhost` vs Host: apps.localhost:80, and the reverse.
    let (want_name, want_port) = split_host_port(&want);
    let (got_name, got_port) = split_host_port(&got);
    if want_name != got_name {
        return false;
    }
    let default = if origin.starts_with("https://") { 443 } else { 80 };
    want_port.unwrap_or(default) == got_port.unwrap_or(default)
}

fn env_path(key: &str) -> Option<PathBuf> {
    std::env::var(key).ok().filter(|s| !s.is_empty()).map(PathBuf::from)
}
