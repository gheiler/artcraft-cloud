//! Mail: the connection to a Stalwart server (super admin), its domains, the mailboxes of
//! artcraftCloud users, and what MailCraft needs to open a user's mailbox.
//!
//! MailCraft's browser build reads JMAP. The shell serves it with `window.mailcraftJmap` naming
//! the user's mailbox at `/jmap/session` on this origin, and a `secret()` that asks
//! `POST /api/mail/token` for a short-lived token. `/jmap/*` checks that token and forwards the
//! request to Stalwart with the mailbox's own credential, which never reaches the browser.

use axum::body::Body;
use axum::extract::{Path, Request, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::Value;

use super::session::AuthUser;
use super::users::{require_admin, require_super};
use crate::db::{Account, MailConfigRow, MailboxRow};
use crate::error::{ApiError, Error};
use crate::password::{sign, verify_mac};
use crate::paths::now_secs;
use crate::stalwart::{normalize_base, Auth, Stalwart};
use crate::state::AppState;
use crate::vault;
use artcraft_cloud_types::{MailConfig, MailConfigUpdate, MailDomain, MailTest, NewMailDomain, RemoveMailbox, SetUserEmail, UserMail};

const CONFIG_CONTEXT: &str = "mail_config";
const TOKEN_TTL: i64 = 15 * 60;
/// Largest request MailCraft sends through the proxy (an upload of a message to send).
const PROXY_BODY_LIMIT: usize = 64 * 1024 * 1024;

fn mailbox_context(user_id: i64) -> String {
    format!("mailbox:{user_id}")
}

fn bad_gateway(message: impl Into<String>) -> ApiError {
    ApiError::new(StatusCode::BAD_GATEWAY, message)
}

fn not_configured() -> ApiError {
    ApiError::new(StatusCode::CONFLICT, "no mail server is configured (Mail in the sidebar)")
}

/// The configured server, with its admin credential, when there is one.
fn server(state: &AppState) -> Result<Option<Stalwart>, ApiError> {
    let Some(row) = state.db.mail_config().map_err(ApiError::from)? else {
        return Ok(None);
    };
    let secret = match &row.secret {
        Some(sealed) => vault::open(&state.secret, CONFIG_CONTEXT, sealed).map_err(ApiError::from)?,
        None => String::new(),
    };
    let auth = if row.auth == "bearer" { Auth::Bearer(secret) } else { Auth::Basic { user: row.username.clone(), pass: secret } };
    Stalwart::new(&row.base_url, auth).map(Some).map_err(bad_gateway)
}

fn view(state: &AppState, row: Option<&MailConfigRow>) -> MailConfig {
    MailConfig {
        base_url: row.map(|r| r.base_url.clone()).unwrap_or_default(),
        auth: row.map(|r| r.auth.clone()).unwrap_or_else(|| "basic".into()),
        username: row.map(|r| r.username.clone()).unwrap_or_default(),
        has_secret: row.is_some_and(|r| r.secret.is_some()),
        suggested_url: state.config.mail_url.clone().unwrap_or_default(),
    }
}

/// `GET /api/mail/config`
pub async fn get_config(State(state): State<AppState>, AuthUser(user): AuthUser) -> Result<Json<MailConfig>, ApiError> {
    require_super(&user).map_err(|_| ApiError::new(StatusCode::FORBIDDEN, "only the super admin sets up mail"))?;
    let row = state.db.mail_config().map_err(ApiError::from)?;
    Ok(Json(view(&state, row.as_ref())))
}

/// `PUT /api/mail/config`: store the address and credential. An empty secret keeps the stored one.
pub async fn put_config(State(state): State<AppState>, AuthUser(user): AuthUser, Json(body): Json<MailConfigUpdate>) -> Result<Json<MailConfig>, ApiError> {
    require_super(&user).map_err(|_| ApiError::new(StatusCode::FORBIDDEN, "only the super admin sets up mail"))?;
    let base_url = normalize_base(&body.base_url).map_err(|err| ApiError::new(StatusCode::BAD_REQUEST, err))?;
    let auth = match body.auth.as_str() {
        "basic" | "bearer" => body.auth.clone(),
        _ => return Err(ApiError::new(StatusCode::BAD_REQUEST, "auth must be basic or bearer")),
    };
    let username = body.username.trim().to_string();
    if auth == "basic" && username.is_empty() {
        return Err(ApiError::new(StatusCode::BAD_REQUEST, "the admin account name is missing"));
    }
    if username.len() > 320 || body.secret.as_deref().is_some_and(|s| s.len() > 4096) {
        return Err(ApiError::new(StatusCode::BAD_REQUEST, "too long"));
    }
    let old = state.db.mail_config().map_err(ApiError::from)?;
    let secret = match body.secret.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        Some(secret) => Some(vault::seal(&state.secret, CONFIG_CONTEXT, secret).map_err(ApiError::from)?),
        None => old.as_ref().filter(|old| old.auth == auth).and_then(|old| old.secret.clone()),
    };
    let row = MailConfigRow { base_url, auth, username: if body.auth == "bearer" { String::new() } else { username }, secret };
    state.db.set_mail_config(&row).map_err(ApiError::from)?;
    tracing::info!(by = %user.username, url = %row.base_url, "mail server configured");
    Ok(Json(view(&state, Some(&row))))
}

/// `DELETE /api/mail/config`: forget the connection (mailboxes on the server stay).
pub async fn delete_config(State(state): State<AppState>, AuthUser(user): AuthUser) -> Result<StatusCode, ApiError> {
    require_super(&user).map_err(|_| ApiError::new(StatusCode::FORBIDDEN, "only the super admin sets up mail"))?;
    state.db.clear_mail_config().map_err(ApiError::from)?;
    Ok(StatusCode::NO_CONTENT)
}

/// `POST /api/mail/test`
pub async fn test(State(state): State<AppState>, AuthUser(user): AuthUser) -> Result<Json<MailTest>, ApiError> {
    require_super(&user).map_err(|_| ApiError::new(StatusCode::FORBIDDEN, "only the super admin sets up mail"))?;
    let server = server(&state)?.ok_or_else(not_configured)?;
    Ok(Json(server.test().await))
}

/// `GET /api/mail/domains`: the server's domains with the DNS records each needs.
pub async fn domains(State(state): State<AppState>, AuthUser(user): AuthUser) -> Result<Json<Vec<MailDomain>>, ApiError> {
    require_admin(&user)?;
    let server = server(&state)?.ok_or_else(not_configured)?;
    let mut list = server.domains().await.map_err(bad_gateway)?;
    if !user.is_super {
        // Admins see which domains are served (for user emails); the records are the super admin's.
        list.iter_mut().for_each(|domain| domain.records.clear());
    }
    Ok(Json(list))
}

/// `POST /api/mail/domains`
pub async fn add_domain(State(state): State<AppState>, AuthUser(user): AuthUser, Json(body): Json<NewMailDomain>) -> Result<(StatusCode, Json<MailDomain>), ApiError> {
    require_super(&user).map_err(|_| ApiError::new(StatusCode::FORBIDDEN, "only the super admin sets up mail"))?;
    let name = normalize_domain(&body.name).ok_or_else(|| ApiError::new(StatusCode::BAD_REQUEST, "that is not a domain name"))?;
    let server = server(&state)?.ok_or_else(not_configured)?;
    if server.domain_id(&name).await.map_err(bad_gateway)?.is_some() {
        return Err(ApiError::new(StatusCode::CONFLICT, "the mail server already serves that domain"));
    }
    let id = server.add_domain(&name).await.map_err(bad_gateway)?;
    let domain = server.domains().await.map_err(bad_gateway)?.into_iter().find(|d| d.id == id).unwrap_or(MailDomain { id, name, records: Vec::new() });
    tracing::info!(by = %user.username, domain = %domain.name, "mail domain added");
    Ok((StatusCode::CREATED, Json(domain)))
}

pub fn normalize_domain(name: &str) -> Option<String> {
    let name = name.trim().trim_end_matches('.').to_ascii_lowercase();
    let ok = name.len() <= 253
        && name.contains('.')
        && name.split('.').all(|label| {
            !label.is_empty() && label.len() <= 63 && !label.starts_with('-') && !label.ends_with('-') && label.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
        });
    ok.then_some(name)
}

/// `local@domain`, lowercased, or why not.
pub fn normalize_email(email: &str) -> Result<(String, String, String), ApiError> {
    let bad = || ApiError::new(StatusCode::BAD_REQUEST, "that is not an email address");
    let email = email.trim().to_ascii_lowercase();
    let (local, domain) = email.split_once('@').ok_or_else(bad)?;
    let domain = normalize_domain(domain).ok_or_else(bad)?;
    let local_ok = !local.is_empty()
        && local.len() <= 64
        && !local.starts_with('.')
        && !local.ends_with('.')
        && !local.contains("..")
        && local.bytes().all(|b| b.is_ascii_alphanumeric() || b"._+-".contains(&b));
    if !local_ok {
        return Err(bad());
    }
    Ok((format!("{local}@{domain}"), local.to_string(), domain))
}

fn target(state: &AppState, actor: &Account, id: i64) -> Result<Account, ApiError> {
    require_admin(actor)?;
    let target = state.db.user_by_id(id).map_err(ApiError::from)?.ok_or_else(|| ApiError::new(StatusCode::NOT_FOUND, "not found"))?;
    if target.is_admin() && actor.id != id {
        require_super(actor)?;
    }
    Ok(target)
}

async fn user_mail(state: &AppState, user_id: i64) -> Result<UserMail, ApiError> {
    let email = state.db.user_email(user_id).map_err(ApiError::from)?;
    let mailbox = state.db.mailbox(user_id).map_err(ApiError::from)?.map(|row| row.address);
    let server = server(state)?;
    let domain_served = match (&server, &email) {
        (Some(server), Some(email)) => match email.split_once('@') {
            Some((_, domain)) => server.domain_id(domain).await.ok().flatten().is_some(),
            None => false,
        },
        _ => false,
    };
    Ok(UserMail { email, mailbox, configured: server.is_some(), domain_served })
}

/// `GET /api/users/{id}/mail`
pub async fn get_user_mail(State(state): State<AppState>, AuthUser(actor): AuthUser, Path(id): Path<i64>) -> Result<Json<UserMail>, ApiError> {
    target(&state, &actor, id)?;
    Ok(Json(user_mail(&state, id).await?))
}

/// `GET /api/mail/me`
pub async fn me(State(state): State<AppState>, AuthUser(user): AuthUser) -> Result<Json<UserMail>, ApiError> {
    let email = state.db.user_email(user.id).map_err(ApiError::from)?;
    let mailbox = state.db.mailbox(user.id).map_err(ApiError::from)?.map(|row| row.address);
    let configured = state.db.mail_config().map_err(ApiError::from)?.is_some();
    Ok(Json(UserMail { email, mailbox, configured, domain_served: false }))
}

/// Set a user's email; with `create_mailbox`, also make the mailbox on the mail server.
pub async fn set_email(state: &AppState, target: &Account, body: &SetUserEmail) -> Result<UserMail, ApiError> {
    let email = match body.email.as_deref().map(str::trim).filter(|e| !e.is_empty()) {
        Some(email) => Some(normalize_email(email)?),
        None => None,
    };
    let linked = state.db.mailbox(target.id).map_err(ApiError::from)?;
    if let Some(linked) = &linked {
        if email.as_ref().map(|e| e.0.as_str()) != Some(linked.address.as_str()) {
            return Err(ApiError::new(StatusCode::CONFLICT, format!("{} has a mailbox; unlink or delete it before changing the email", target.username)));
        }
    }
    let previous = state.db.user_email(target.id).map_err(ApiError::from)?;
    match state.db.set_user_email(target.id, email.as_ref().map(|e| e.0.as_str())) {
        Ok(()) => {}
        Err(Error::Exists) => return Err(ApiError::new(StatusCode::CONFLICT, "another user has that email")),
        Err(err) => return Err(err.into()),
    }
    if body.create_mailbox && linked.is_none() {
        let Some((address, local, domain)) = email else {
            return Err(ApiError::new(StatusCode::BAD_REQUEST, "an email is needed for a mailbox"));
        };
        if let Err(err) = provision(state, target, &address, &local, &domain).await {
            let _ = state.db.set_user_email(target.id, previous.as_deref());
            return Err(err);
        }
    }
    user_mail(state, target.id).await
}

/// Create `local@domain` on the mail server with a generated password, and link it.
async fn provision(state: &AppState, target: &Account, address: &str, local: &str, domain: &str) -> Result<(), ApiError> {
    let server = server(state)?.ok_or_else(not_configured)?;
    let domain_id = server
        .domain_id(domain)
        .await
        .map_err(bad_gateway)?
        .ok_or_else(|| ApiError::new(StatusCode::CONFLICT, format!("the mail server does not serve {domain}; add the domain under Mail first")))?;
    if server.find_account(local, &domain_id).await.map_err(bad_gateway)?.is_some() {
        return Err(ApiError::new(StatusCode::CONFLICT, format!("{address} already exists on the mail server; artcraftCloud links only mailboxes it creates")));
    }
    let password = crate::state::generate_password() + &crate::state::generate_password();
    let account_id = server.create_user(local, &domain_id, &target.display_name, &password).await.map_err(bad_gateway)?;
    let sealed = vault::seal(&state.secret, &mailbox_context(target.id), &password).map_err(ApiError::from)?;
    let row = MailboxRow { user_id: target.id, address: address.to_string(), account_id: account_id.clone(), secret: sealed };
    if let Err(err) = state.db.link_mailbox(&row) {
        let _ = server.destroy_account(&account_id).await;
        return Err(match err {
            Error::Exists => ApiError::new(StatusCode::CONFLICT, "that mailbox is linked to another user"),
            other => other.into(),
        });
    }
    tracing::info!(user = %target.username, %address, "mailbox created");
    Ok(())
}

/// `PUT /api/users/{id}/email`
pub async fn put_user_email(State(state): State<AppState>, AuthUser(actor): AuthUser, Path(id): Path<i64>, Json(body): Json<SetUserEmail>) -> Result<Json<UserMail>, ApiError> {
    let target = target(&state, &actor, id)?;
    Ok(Json(set_email(&state, &target, &body).await?))
}

/// `DELETE /api/users/{id}/mailbox`: unlink (the mailbox stays on the server), or with
/// `delete` and the address typed again as `confirm`, delete it on the server as well.
pub async fn remove_mailbox(State(state): State<AppState>, AuthUser(actor): AuthUser, Path(id): Path<i64>, Json(body): Json<RemoveMailbox>) -> Result<Json<UserMail>, ApiError> {
    let target = target(&state, &actor, id)?;
    let linked = state.db.mailbox(id).map_err(ApiError::from)?.ok_or_else(|| ApiError::new(StatusCode::NOT_FOUND, "no mailbox is linked"))?;
    if body.delete {
        if !body.confirm.trim().eq_ignore_ascii_case(&linked.address) {
            return Err(ApiError::new(StatusCode::BAD_REQUEST, "type the mailbox address to delete it"));
        }
        let server = server(&state)?.ok_or_else(not_configured)?;
        server.destroy_account(&linked.account_id).await.map_err(bad_gateway)?;
        tracing::warn!(by = %actor.username, user = %target.username, address = %linked.address, "mailbox deleted on the mail server");
    } else {
        tracing::info!(by = %actor.username, user = %target.username, address = %linked.address, "mailbox unlinked");
    }
    state.db.unlink_mailbox(id).map_err(ApiError::from)?;
    Ok(Json(user_mail(&state, id).await?))
}

fn token_message(user_id: i64, expires: i64) -> String {
    format!("mailcraft-jmap:{user_id}:{expires}")
}

pub fn mint_token(secret: &[u8], user_id: i64, now: i64) -> Result<String, Error> {
    let expires = now + TOKEN_TTL;
    Ok(format!("mc1.{user_id}.{expires}.{}", sign(secret, &token_message(user_id, expires))?))
}

pub fn check_token(secret: &[u8], token: &str, now: i64) -> Option<i64> {
    let mut parts = token.strip_prefix("mc1.")?.splitn(3, '.');
    let user_id: i64 = parts.next()?.parse().ok()?;
    let expires: i64 = parts.next()?.parse().ok()?;
    let mac = parts.next()?;
    (expires >= now && expires <= now + TOKEN_TTL && verify_mac(secret, &token_message(user_id, expires), mac)).then_some(user_id)
}

/// `POST /api/mail/token`: what MailCraft's `secret()` hands to the JMAP proxy.
pub async fn token(State(state): State<AppState>, AuthUser(user): AuthUser) -> Result<Response, ApiError> {
    if state.db.mailbox(user.id).map_err(ApiError::from)?.is_none() {
        return Err(ApiError::new(StatusCode::NOT_FOUND, "no mailbox is linked"));
    }
    let token = mint_token(&state.secret, user.id, now_secs()).map_err(ApiError::from)?;
    Ok(([(header::CACHE_CONTROL, "no-store")], Json(serde_json::json!({ "token": token, "expiresIn": TOKEN_TTL }))).into_response())
}

/// `GET /api/mail/mailcraft.js`: `window.mailcraftJmap` for the signed-in user, loaded by
/// MailCraft's page before the app starts.
pub async fn mailcraft_hook(State(state): State<AppState>, AuthUser(user): AuthUser) -> Result<Response, ApiError> {
    let script = match state.db.mailbox(user.id).map_err(ApiError::from)? {
        Some(mailbox) => {
            let address = serde_json::to_string(&mailbox.address).unwrap_or_default();
            let name = serde_json::to_string(&format!("{} (artcraftCloud)", user.display_name)).unwrap_or_default();
            format!(
                "// artcraftCloud: the signed-in user's mailbox, through this server's JMAP proxy.\n\
                 window.mailcraftJmap = {{\n  accounts: [{{ email: {address}, session: location.origin + \"/jmap/session\", bearer: true, name: {name} }}],\n  \
                 secret: function () {{\n    return fetch(\"/api/mail/token\", {{ method: \"POST\", credentials: \"same-origin\", headers: {{ \"x-requested-with\": \"artcraft\" }} }})\n      \
                 .then(function (r) {{ return r.ok ? r.json() : {{}}; }})\n      .then(function (j) {{ return j.token || \"\"; }});\n  }}\n}};\n"
            )
        }
        None => "// artcraftCloud: no mailbox is linked to this account.\n".to_string(),
    };
    Ok(([(header::CONTENT_TYPE, "text/javascript; charset=utf-8"), (header::CACHE_CONTROL, "no-store")], script).into_response())
}

/// MailCraft's `index.html` with the hook script in front of the app's own scripts.
pub fn inject_hook(html: &str) -> String {
    const TAG: &str = "<script src=\"/api/mail/mailcraft.js\"></script>";
    let lower = html.to_ascii_lowercase();
    match lower.find("<head") .and_then(|at| lower[at..].find('>').map(|end| at + end + 1)) {
        Some(at) => format!("{}{TAG}{}", &html[..at], &html[at..]),
        None => format!("{TAG}{html}"),
    }
}

/// Session URLs as paths, so they resolve against this origin (MailCraft sends the credential
/// only to the session's own origin); and no WebSocket (the proxy speaks plain HTTP).
pub fn rewrite_session(mut session: Value) -> Value {
    for key in ["apiUrl", "downloadUrl", "uploadUrl", "eventSourceUrl"] {
        if let Some(url) = session.get(key).and_then(Value::as_str) {
            let path = match url.find("://") {
                Some(scheme) => url[scheme + 3..].find('/').map(|slash| url[scheme + 3 + slash..].to_string()).unwrap_or_else(|| "/".into()),
                None if url.starts_with('/') => url.to_string(),
                None => format!("/{url}"),
            };
            session[key] = Value::String(path);
        }
    }
    if let Some(caps) = session.get_mut("capabilities").and_then(Value::as_object_mut) {
        caps.remove("urn:ietf:params:jmap:websocket");
    }
    session
}

fn unauthorized_bearer() -> Response {
    let mut response = ApiError::unauthorized().into_response();
    response.headers_mut().insert(header::WWW_AUTHENTICATE, HeaderValue::from_static("Bearer realm=\"artcraftCloud mail\""));
    response
}

/// `/jmap/*`: MailCraft's JMAP requests, forwarded to the mail server as the user's mailbox.
pub async fn jmap_proxy(State(state): State<AppState>, req: Request) -> Response {
    match proxy(&state, req).await {
        Ok(response) => response,
        Err(response) => response,
    }
}

// The error is the response itself (a 401 with its challenge, a 502…), built once per request.
#[allow(clippy::result_large_err)]
async fn proxy(state: &AppState, req: Request) -> Result<Response, Response> {
    let bearer = req.headers().get(header::AUTHORIZATION).and_then(|v| v.to_str().ok()).and_then(|v| v.strip_prefix("Bearer ")).map(str::trim).unwrap_or("");
    let user_id = check_token(&state.secret, bearer, now_secs()).ok_or_else(unauthorized_bearer)?;
    let user = state.db.user_by_id(user_id).ok().flatten().filter(|u| !u.disabled).ok_or_else(unauthorized_bearer)?;
    let mailbox = state.db.mailbox(user.id).ok().flatten().ok_or_else(unauthorized_bearer)?;
    let config = state.db.mail_config().ok().flatten().ok_or_else(|| not_configured().into_response())?;
    let password = vault::open(&state.secret, &mailbox_context(user.id), &mailbox.secret).map_err(|err| ApiError::from(err).into_response())?;
    let path = req.uri().path().to_string();
    if !proxied_path(&path) {
        return Err(ApiError::new(StatusCode::BAD_REQUEST, "bad path").into_response());
    }
    let query = req.uri().query().map(|q| format!("?{q}")).unwrap_or_default();
    let method = req.method().clone();
    let headers = req.headers().clone();
    let body = axum::body::to_bytes(req.into_body(), PROXY_BODY_LIMIT)
        .await
        .map_err(|_| ApiError::new(StatusCode::PAYLOAD_TOO_LARGE, "request is too large").into_response())?;
    let mut upstream = crate::stalwart::http()
        .request(method.clone(), format!("{}{path}{query}", config.base_url))
        .basic_auth(&mailbox.address, Some(&password))
        .timeout(std::time::Duration::from_secs(120));
    for name in [header::CONTENT_TYPE, header::ACCEPT, header::RANGE] {
        if let Some(value) = headers.get(&name) {
            upstream = upstream.header(name, value);
        }
    }
    if !body.is_empty() {
        upstream = upstream.body(body);
    }
    let reply = upstream.send().await.map_err(|_| bad_gateway("the mail server could not be reached").into_response())?;
    let status = StatusCode::from_u16(reply.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
    if status == StatusCode::UNAUTHORIZED {
        tracing::warn!(user = %user.username, "the mail server refused a linked mailbox's password");
        return Err(unauthorized_bearer());
    }
    let mut out = HeaderMap::new();
    for name in [header::CONTENT_TYPE, header::CONTENT_DISPOSITION, header::RETRY_AFTER, header::CONTENT_RANGE, header::ACCEPT_RANGES] {
        if let Some(value) = reply.headers().get(name.as_str()).and_then(|v| HeaderValue::from_bytes(v.as_bytes()).ok()) {
            out.insert(name, value);
        }
    }
    out.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    let is_session = method == axum::http::Method::GET && (path == "/jmap/session" || path == "/jmap/session/");
    if is_session && status.is_success() {
        let session: Value = reply.json().await.map_err(|_| bad_gateway("the mail server's session is not JSON").into_response())?;
        let mut response = Json(rewrite_session(session)).into_response();
        response.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
        return Ok(response);
    }
    let mut response = Response::new(Body::from_stream(reply.bytes_stream()));
    *response.status_mut() = status;
    *response.headers_mut() = out;
    Ok(response)
}

/// Only Stalwart's JMAP resources, never a path that would climb out of `/jmap` upstream.
fn proxied_path(path: &str) -> bool {
    (path == "/jmap" || path.starts_with("/jmap/"))
        && !path.split('/').any(|segment| segment == ".." || segment == ".")
        && !path.to_ascii_lowercase().contains("%2e")
        && !path.contains('\\')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_proxy_stays_under_jmap() {
        for ok in ["/jmap", "/jmap/", "/jmap/session", "/jmap/download/d/b/message.eml", "/jmap/upload/d/"] {
            assert!(proxied_path(ok), "{ok}");
        }
        for bad in ["/jmap/../api/account", "/jmap/./x", "/jmap/%2e%2e/api", "/jmap/%2E%2E/api", "/jmapx", "/api/account", "/jmap/a\\..\\b"] {
            assert!(!proxied_path(bad), "{bad}");
        }
    }

    #[test]
    fn tokens_name_their_user_expire_and_cannot_be_forged() {
        let key = [3u8; 32];
        let token = mint_token(&key, 7, 1000).unwrap();
        assert_eq!(check_token(&key, &token, 1000), Some(7));
        assert_eq!(check_token(&key, &token, 1000 + TOKEN_TTL + 1), None, "expired");
        assert_eq!(check_token(&[4u8; 32], &token, 1000), None, "another key");
        let forged = token.replacen("mc1.7.", "mc1.8.", 1);
        assert_eq!(check_token(&key, &forged, 1000), None, "another user");
        assert_eq!(check_token(&key, "mc1.7.99999999999.00", 1000), None);
        assert_eq!(check_token(&key, "", 1000), None);
    }

    #[test]
    fn sessions_point_at_this_origin_without_websocket() {
        let session = serde_json::json!({
            "apiUrl": "https://mail.example.org/jmap/",
            "downloadUrl": "/jmap/download/{accountId}/{blobId}/{name}?accept={type}",
            "uploadUrl": "jmap/upload/{accountId}/",
            "eventSourceUrl": "http://stalwart:8080/jmap/eventsource/?types={types}",
            "capabilities": {"urn:ietf:params:jmap:core": {}, "urn:ietf:params:jmap:websocket": {"url": "ws/jmap/ws"}},
        });
        let out = rewrite_session(session);
        assert_eq!(out["apiUrl"], "/jmap/");
        assert_eq!(out["downloadUrl"], "/jmap/download/{accountId}/{blobId}/{name}?accept={type}");
        assert_eq!(out["uploadUrl"], "/jmap/upload/{accountId}/");
        assert_eq!(out["eventSourceUrl"], "/jmap/eventsource/?types={types}");
        assert!(out["capabilities"].get("urn:ietf:params:jmap:websocket").is_none());
    }

    #[test]
    fn emails_and_domains_are_checked() {
        assert_eq!(normalize_email(" Ada.L@Example.ORG ").ok().map(|e| e.0), Some("ada.l@example.org".into()));
        for bad in ["ada", "@x.org", "ada@", "a b@x.org", "ada@x", "ada@-x.org", ".a@x.org", "a..b@x.org", "a<b>@x.org"] {
            assert!(normalize_email(bad).is_err(), "{bad}");
        }
        assert_eq!(normalize_domain("Mail.Example.org."), Some("mail.example.org".into()));
        assert_eq!(normalize_domain("localhost"), None);
    }

    #[test]
    fn the_hook_goes_first_in_head() {
        let html = "<!doctype html><html><HEAD lang=x><script src=app.js></script></head></html>";
        let out = inject_hook(html);
        assert!(out.starts_with("<!doctype html><html><HEAD lang=x><script src=\"/api/mail/mailcraft.js\"></script><script src=app.js>"));
    }
}
