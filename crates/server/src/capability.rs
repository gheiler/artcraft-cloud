//! Short-lived capability tokens the shell issues to a sideloaded app over the postMessage
//! bridge. A token is bound to one user and one app, and is either scoped to a single file
//! (read and write) or create-only (the app may ask the shell to save a new file; it cannot
//! read existing files). An opaque-origin (`Origin: null`) request may use a token, never the
//! session cookie.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::paths::random_token;

const TTL_SECS: u64 = 30 * 60;

#[derive(Clone, Debug)]
pub struct Capability {
    pub token: String,
    pub user_id: i64,
    pub app_id: String,
    /// `Some` = read and write this drive path only. `None` = create-only.
    pub path: Option<String>,
    pub expires: u64,
}

#[derive(Default)]
pub struct CapabilityStore {
    inner: Mutex<HashMap<String, Capability>>,
}

#[derive(Debug, Deserialize)]
pub struct IssueRequest {
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub create: bool,
}

#[derive(Debug, Serialize)]
pub struct IssueResponse {
    pub token: String,
    pub expires: u64,
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

impl CapabilityStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn issue(&self, user_id: i64, app_id: &str, path: Option<String>) -> IssueResponse {
        let expires = now().saturating_add(TTL_SECS);
        let token = random_token(32);
        let cap = Capability {
            token: token.clone(),
            user_id,
            app_id: app_id.to_string(),
            path,
            expires,
        };
        let mut map = self.inner.lock().expect("capability lock");
        map.retain(|_, item| item.expires > now());
        map.insert(token.clone(), cap);
        IssueResponse { token, expires }
    }

    pub fn lookup(&self, token: &str) -> Option<Capability> {
        let now = now();
        let map = self.inner.lock().expect("capability lock");
        map.get(token).filter(|item| item.expires > now).cloned()
    }
}

impl Capability {
    pub fn allows_path(&self, path: &str) -> bool {
        self.path.as_deref() == Some(path)
    }

    pub fn can_write(&self, path: &str) -> bool {
        self.allows_path(path)
    }

    pub fn can_read(&self, path: &str) -> bool {
        self.allows_path(path)
    }
}

/// The `Authorization` bearer token, if any.
pub fn bearer(headers: &axum::http::HeaderMap) -> Option<&str> {
    let value = headers.get(axum::http::header::AUTHORIZATION)?.to_str().ok()?;
    value.strip_prefix("Bearer ").or_else(|| value.strip_prefix("bearer "))
}

pub fn origin_is_null(headers: &axum::http::HeaderMap) -> bool {
    headers
        .get(axum::http::header::ORIGIN)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.eq_ignore_ascii_case("null"))
}
