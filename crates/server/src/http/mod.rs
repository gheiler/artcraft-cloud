mod files;
pub(crate) mod mail;
mod session;
mod shares;
pub(crate) mod users;

use axum::extract::{Request, State};
use axum::http::{header, HeaderValue, Method, StatusCode};
use axum::middleware::{from_fn, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{any, get, post};
use axum::{Json, Router};

use crate::error::{ApiError, Error};
use crate::manifest::current_dir;
use crate::state::AppState;
use crate::static_files::{self, CachePolicy};
use artcraft_cloud_types::HealthBody;

pub fn router(state: AppState) -> Router {
    let limit = state.config.max_upload_bytes as usize;
    Router::new()
        .route("/healthz", get(health))
        .route("/readyz", get(ready))
        .route("/api/session", post(session::login).delete(session::logout))
        .route("/api/me", get(session::me))
        .route("/api/me/password", post(session::change_password))
        .route("/api/users", get(users::list_users).post(users::create_user))
        .route("/api/users/{id}/disable", post(users::disable_user))
        .route("/api/users/{id}/enable", post(users::enable_user))
        .route("/api/users/{id}/password", post(users::reset_password))
        .route("/api/users/{id}/role", post(users::set_role))
        .route("/api/users/{id}/mail", get(mail::get_user_mail))
        .route("/api/users/{id}/email", axum::routing::put(mail::put_user_email))
        .route("/api/users/{id}/mailbox", axum::routing::delete(mail::remove_mailbox))
        .route("/api/mail/config", get(mail::get_config).put(mail::put_config).delete(mail::delete_config))
        .route("/api/mail/test", post(mail::test))
        .route("/api/mail/domains", get(mail::domains).post(mail::add_domain))
        .route("/api/mail/me", get(mail::me))
        .route("/api/mail/token", post(mail::token))
        .route("/api/mail/mailcraft.js", get(mail::mailcraft_hook))
        .route("/jmap", any(mail::jmap_proxy))
        .route("/jmap/", any(mail::jmap_proxy))
        .route("/jmap/{*rest}", any(mail::jmap_proxy))
        .route("/api/directory", get(users::directory))
        .route("/api/groups", get(users::list_groups).post(users::create_group))
        .route("/api/groups/{id}", axum::routing::delete(users::delete_group))
        .route("/api/groups/{id}/members", post(users::add_member))
        .route("/api/groups/{id}/members/{username}", axum::routing::delete(users::remove_member))
        .route("/api/settings/{app}", get(users::get_settings).put(users::put_settings))
        .route("/api/preferences", get(users::get_preferences).put(users::put_preferences))
        .route("/api/files/kind", get(files::file_kind))
        .route("/api/files", get(files::list_files))
        .route("/api/files/content", get(files::read_file).put(files::write_file))
        .route("/api/files/upload", post(files::upload))
        .route("/api/files/mkdir", post(files::mkdir))
        .route("/api/files/rename", post(files::rename))
        .route("/api/files/move", post(files::move_path))
        .route("/api/files/copy", post(files::copy_path))
        .route("/api/files/delete", post(files::delete_path))
        .route("/api/trash", get(files::list_trash).delete(files::empty_trash))
        .route("/api/trash/{id}/restore", post(files::restore_trash))
        .route("/api/trash/{id}", axum::routing::delete(files::purge_trash))
        .route("/api/shares", get(shares::list_shares).post(shares::create_share))
        .route("/api/shares/{id}", axum::routing::delete(shares::delete_share))
        .route("/api/shares/{id}/list", get(shares::list_shared))
        .route("/api/shares/{id}/content", get(shares::read_shared).put(shares::write_shared))
        .route("/api/public/{token}", get(shares::public_view))
        .route("/api/public/{token}/unlock", post(shares::public_unlock))
        .route("/api/public/{token}/content", get(shares::public_read).put(shares::public_write))
        .route("/api/apps", get(users::apps))
        .route("/apps", get(serve_app_index))
        .route("/apps/", get(serve_app_index))
        .route("/apps/{*path}", get(serve_app))
        .route("/dav", any(crate::webdav::dav))
        .route("/dav/", any(crate::webdav::dav))
        .route("/dav/{*path}", any(crate::webdav::dav))
        .fallback(shell)
        .layer(from_fn(csrf))
        .layer(axum::extract::DefaultBodyLimit::max(limit))
        .with_state(state)
}

async fn csrf(req: Request, next: Next) -> Response {
    let method = req.method().clone();
    let path = req.uri().path().to_string();
    if matches!(method, Method::POST | Method::PUT | Method::PATCH | Method::DELETE) && path.starts_with("/api/") {
        let ok = req.headers().get("x-requested-with").and_then(|v| v.to_str().ok()) == Some("artcraft");
        if !ok {
            return ApiError::new(StatusCode::FORBIDDEN, "missing X-Requested-With: artcraft").into_response();
        }
        if let Some(len) = req.headers().get(header::CONTENT_LENGTH).and_then(|v| v.to_str().ok()).and_then(|v| v.parse::<u64>().ok()) {
            let upload = path.contains("/content") || path.ends_with("/upload");
            if !upload && len > 256 * 1024 {
                return ApiError::new(StatusCode::PAYLOAD_TOO_LARGE, "request is too large").into_response();
            }
        }
    }
    next.run(req).await
}

async fn health() -> Json<HealthBody> {
    Json(HealthBody { status: "ok".into(), version: env!("CARGO_PKG_VERSION").into() })
}

async fn ready(State(state): State<AppState>) -> Result<Json<HealthBody>, ApiError> {
    state.db.ping().map_err(ApiError::from)?;
    Ok(Json(HealthBody { status: "ready".into(), version: env!("CARGO_PKG_VERSION").into() }))
}

async fn serve_app_index(State(state): State<AppState>, req: Request) -> Response {
    serve_app_at(&state, "", req.headers()).await
}

async fn serve_app(State(state): State<AppState>, axum::extract::Path(path): axum::extract::Path<String>, req: Request) -> Response {
    serve_app_at(&state, &path, req.headers()).await
}

async fn serve_app_at(state: &AppState, path: &str, headers: &axum::http::HeaderMap) -> Response {
    let mut parts = path.splitn(2, '/');
    let Some(id) = parts.next().filter(|id| !id.is_empty()) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    if artcraft_cloud_types::app_by_id(id).is_none() {
        return StatusCode::NOT_FOUND.into_response();
    }
    let Some(root) = current_dir(&state.config.data_dir, id) else {
        return (StatusCode::SERVICE_UNAVAILABLE, format!("{id} is not installed yet")).into_response();
    };
    let rest = parts.next().unwrap_or("");
    if id == "mailcraft" && (rest.is_empty() || rest == "index.html") {
        // MailCraft learns the user's mailbox from a script that must run before the app.
        let Ok(html) = tokio::fs::read_to_string(root.join("index.html")).await else {
            return StatusCode::NOT_FOUND.into_response();
        };
        let mut response = ([(header::CONTENT_TYPE, "text/html; charset=utf-8"), (header::CACHE_CONTROL, "no-store")], mail::inject_hook(&html)).into_response();
        response.headers_mut().insert("content-security-policy", csp());
        response.headers_mut().insert(header::X_FRAME_OPTIONS, HeaderValue::from_static("SAMEORIGIN"));
        return response;
    }
    let mut response = static_files::serve_path(&root, rest, headers, CachePolicy::Immutable).await;
    let headers = response.headers_mut();
    headers.insert("content-security-policy", csp());
    headers.insert(header::X_FRAME_OPTIONS, HeaderValue::from_static("SAMEORIGIN"));
    response
}

async fn shell(State(state): State<AppState>, req: Request) -> Response {
    let path = req.uri().path();
    if path.starts_with("/api/") || path.starts_with("/dav") || path.starts_with("/jmap") {
        return StatusCode::NOT_FOUND.into_response();
    }
    let root = &state.config.shell_dir;
    if !root.join("index.html").is_file() {
        return (
            StatusCode::OK,
            [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
            "artcraftCloud API is up. The shell has not been built into ARTCRAFT_SHELL_DIR.",
        )
            .into_response();
    }
    let rel = path.trim_start_matches('/');
    let candidate = static_files::safe_join(root, rel);
    let serve_rel = if candidate.as_ref().is_some_and(|p| p.is_file()) { rel } else { "index.html" };
    let mut response = static_files::serve_path(root, serve_rel, req.headers(), CachePolicy::Html).await;
    response.headers_mut().insert("content-security-policy", csp());
    response
}

fn csp() -> HeaderValue {
    HeaderValue::from_static(
        "default-src 'self'; script-src 'self' 'unsafe-inline' 'unsafe-eval' 'wasm-unsafe-eval'; style-src 'self' 'unsafe-inline'; img-src 'self' blob: data:; font-src 'self' data:; connect-src 'self'; worker-src 'self' blob:; frame-src 'self'; object-src 'none'; base-uri 'self'",
    )
}

pub fn cookie_value(header: &str, name: &str) -> Option<String> {
    header.split(';').map(str::trim).find_map(|part| part.strip_prefix(&format!("{name}=")).map(str::to_string))
}

pub async fn blocking<T: Send + 'static>(work: impl FnOnce() -> Result<T, Error> + Send + 'static) -> Result<T, ApiError> {
    match tokio::task::spawn_blocking(work).await {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(err)) => Err(err.into()),
        Err(_) => Err(ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "worker failed")),
    }
}

pub fn session_cookie(token: &str, ttl: i64, secure: bool) -> String {
    let secure = if secure { "; Secure" } else { "" };
    format!("artcraft_session={token}; HttpOnly; SameSite=Lax; Path=/; Max-Age={ttl}{secure}")
}

pub fn clear_cookie(secure: bool) -> String {
    session_cookie("deleted", 0, secure)
}

