pub(crate) mod apps;
pub(crate) mod calendar;
mod files;
pub(crate) mod mail;
mod session;
mod shares;
pub(crate) mod users;

use axum::extract::{Request, State};
use axum::http::{header, HeaderValue, Method, StatusCode};
use axum::middleware::{from_fn, from_fn_with_state, Next};
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
        .route("/api/calendar/calendarcraft.js", get(calendar::calendarcraft_hook))
        .route("/api/calendar/remote/credentials/{account}", axum::routing::put(calendar::put_remote_credential).delete(calendar::delete_remote_credential))
        .route("/api/calendar/remote", any(calendar::remote_proxy))
        .route("/caldav", any(calendar::caldav_proxy))
        .route("/caldav/", any(calendar::caldav_proxy))
        .route("/caldav/{*rest}", any(calendar::caldav_proxy))
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
        .route("/api/apps", get(apps::list))
        .route("/api/catalog", get(apps::public_catalog))
        .route("/api/apps/{id}/icon", get(apps::icon))
        .route("/api/admin/apps", get(apps::admin_list))
        .route("/api/admin/apps/refresh", post(apps::admin_refresh))
        .route("/api/admin/apps/sideload", post(apps::admin_sideload))
        .route("/api/admin/apps/{id}/source", post(apps::admin_source))
        .route("/api/admin/apps/{id}/check", post(apps::admin_check))
        .route("/api/apps/{id}/ack", post(apps::ack_unverified))
        .route("/api/apps/{id}/capability", post(apps::issue_capability))
        .route("/api/admin/apps/{id}", axum::routing::delete(apps::admin_remove))
        .route("/api/admin/apps/{id}/install", post(apps::admin_install))
        .route("/api/admin/apps/{id}/rollback", post(apps::admin_rollback))
        .route("/api/admin/apps/{id}/settings", axum::routing::put(apps::admin_settings))
        .route("/api/admin/apps/{id}/dismiss", post(apps::admin_dismiss))
        .route("/apps", get(serve_app_index))
        .route("/apps/", get(serve_app_index))
        .route("/apps/{*path}", get(serve_app))
        .route("/dav", any(crate::webdav::dav))
        .route("/dav/", any(crate::webdav::dav))
        .route("/dav/{*path}", any(crate::webdav::dav))
        .fallback(shell)
        .layer(from_fn(csrf))
        .layer(from_fn_with_state(state.clone(), opaque_origin))
        .layer(from_fn_with_state(state.clone(), restrict_apps_host))
        .layer(axum::extract::DefaultBodyLimit::max(limit))
        .with_state(state)
}

/// The apps origin serves only app files (and health checks). It never sets a session cookie and
/// it never answers `/api`, `/dav`, `/jmap` or `/caldav`, so a sideloaded app cannot act as the
/// signed-in person.
async fn restrict_apps_host(State(state): State<AppState>, req: Request, next: Next) -> Response {
    let host = req.headers().get(header::HOST).and_then(|v| v.to_str().ok()).unwrap_or("");
    if !state.config.is_apps_host(host) {
        return next.run(req).await;
    }
    let path = req.uri().path();
    if path == "/healthz" || path == "/readyz" || path.starts_with("/apps/") {
        let mut response = next.run(req).await;
        response.headers_mut().remove(header::SET_COOKIE);
        return response;
    }
    ApiError::new(StatusCode::NOT_FOUND, "not found").into_response()
}

async fn csrf(req: Request, next: Next) -> Response {
    let method = req.method().clone();
    let path = req.uri().path().to_string();
    if matches!(method, Method::POST | Method::PUT | Method::PATCH | Method::DELETE) && path.starts_with("/api/") {
        let requested = req.headers().get("x-requested-with").and_then(|v| v.to_str().ok()) == Some("artcraft");
        let token = path == "/api/files/content" && crate::capability::bearer(req.headers()).is_some();
        if !requested && !token {
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

/// A sandboxed iframe (no allow-same-origin) has an opaque origin and sends `Origin: null`.
/// Those requests may not use the session cookie; only a short-lived capability token, and
/// only for the file that token covers.
async fn opaque_origin(State(state): State<AppState>, req: Request, next: Next) -> Response {
    if !crate::capability::origin_is_null(req.headers()) {
        return next.run(req).await;
    }
    let path = req.uri().path();
    let sensitive = path.starts_with("/api/") || path.starts_with("/dav") || path.starts_with("/jmap") || path.starts_with("/caldav");
    if !sensitive {
        return next.run(req).await;
    }
    if path == "/api/files/content" {
        if let Some(token) = crate::capability::bearer(req.headers()) {
            if state.capabilities.lookup(token).is_some() {
                return next.run(req).await;
            }
        }
    }
    ApiError::new(StatusCode::FORBIDDEN, "opaque origin cannot use the session").into_response()
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
    let host = headers.get(header::HOST).and_then(|v| v.to_str().ok()).unwrap_or("");
    let on_apps = state.config.is_apps_host(host);
    let manifest = crate::manifest::read_manifest(&state.config.data_dir);
    let row = manifest.apps.get(id);
    let isolation = row.and_then(|r| r.sideload.as_ref()).map(crate::sideload::isolation_of);
    let wants_domain = isolation.as_deref() == Some(artcraft_cloud_types::ISOLATION_DOMAIN);
    let sandboxed = isolation.as_deref() == Some(artcraft_cloud_types::ISOLATION_SANDBOX)
        || (wants_domain && state.config.apps_origin().is_none());
    let uses_apps_host = wants_domain && state.config.apps_origin().is_some();
    match row {
        // Installed apps keep being served even if the registry stops listing them; a blocked
        // version is handled by the registry check (rolled back or disabled), not here.
        Some(row) if row.disabled.is_some() => return (StatusCode::FORBIDDEN, format!("{id} is disabled by the administrator")).into_response(),
        Some(_) => {}
        None if apps::registry_app(state, id).is_none() => return StatusCode::NOT_FOUND.into_response(),
        None => {}
    }
    if uses_apps_host && !on_apps {
        if let Some(origin) = state.config.apps_origin() {
            let rest = if path.is_empty() { String::new() } else { format!("/{path}") };
            let loc = format!("{origin}/apps{rest}");
            if let Ok(value) = HeaderValue::from_str(&loc) {
                return (StatusCode::TEMPORARY_REDIRECT, [(header::LOCATION, value)]).into_response();
            }
        }
    }
    // Official and regular sideloads stay on the main host.
    if on_apps && !uses_apps_host {
        return StatusCode::NOT_FOUND.into_response();
    }
    let Some(root) = current_dir(&state.config.data_dir, id) else {
        return (StatusCode::SERVICE_UNAVAILABLE, format!("{id} is not installed yet")).into_response();
    };
    let rest = parts.next().unwrap_or("");
    // Mail and calendar hooks stay official-only and are never injected into a sideload, even
    // one that reused those ids.
    let hook: Option<fn(&str) -> String> = if isolation.is_some() {
        None
    } else {
        match id {
            "mailcraft" => Some(mail::inject_hook),
            "calendarcraft" => Some(calendar::inject_hook),
            _ => None,
        }
    };
    if let Some(inject) = hook.filter(|_| rest.is_empty() || rest == "index.html") {
        // MailCraft and CalendarCraft learn the user's mailbox and calendars from a script that
        // must run before the app.
        let Ok(html) = tokio::fs::read_to_string(root.join("index.html")).await else {
            return StatusCode::NOT_FOUND.into_response();
        };
        return app_response(state, isolation.as_deref(), ([(header::CONTENT_TYPE, "text/html; charset=utf-8"), (header::CACHE_CONTROL, "no-store")], inject(&html)).into_response());
    }
    let mut response = static_files::serve_path(&root, rest, headers, CachePolicy::Immutable).await;
    // Sandboxed (and separate-domain) index pages get the parent origin and a postMessage shim.
    // Regular sideloads are same-origin, so official picker hooks apply and no shim is injected.
    if isolation.is_some() && isolation.as_deref() != Some(artcraft_cloud_types::ISOLATION_REGULAR) && (rest.is_empty() || rest == "index.html") {
        if let (Some(parent), Some(ct)) = (parent_origin(state, headers), response.headers().get(header::CONTENT_TYPE)) {
            if ct.to_str().unwrap_or("").starts_with("text/html") {
                if let Ok(mut html) = tokio::fs::read_to_string(root.join("index.html")).await {
                    if !parent.is_empty() {
                        let shim = if sandboxed { "true" } else { "false" };
                        let tag = format!(
                            "<script>window.__ARTCRAFT_PARENT__={};window.__ARTCRAFT_APP__={};window.__ARTCRAFT_SHIM_STORAGE__={};{}</script>",
                            json_str(&parent),
                            json_str(id),
                            shim,
                            include_str!("../sideload_bridge.js")
                        );
                        if let Some(idx) = html.to_ascii_lowercase().find("<head>") {
                            html.insert_str(idx + 6, &tag);
                        } else {
                            html = tag + &html;
                        }
                    }
                    response = ([(header::CONTENT_TYPE, "text/html; charset=utf-8"), (header::CACHE_CONTROL, "no-store")], html).into_response();
                }
            }
        }
    }
    app_response(state, isolation.as_deref(), response)
}

fn json_str(value: &str) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "\"\"".to_string())
}

fn app_response(state: &AppState, isolation: Option<&str>, mut response: Response) -> Response {
    let headers = response.headers_mut();
    let sandboxed = isolation == Some(artcraft_cloud_types::ISOLATION_SANDBOX)
        || (isolation == Some(artcraft_cloud_types::ISOLATION_DOMAIN) && state.config.apps_origin().is_none());
    let framed = isolation == Some(artcraft_cloud_types::ISOLATION_SANDBOX)
        || isolation == Some(artcraft_cloud_types::ISOLATION_DOMAIN);
    headers.insert(
        "content-security-policy",
        if sandboxed {
            apps_csp(state, true)
        } else if isolation == Some(artcraft_cloud_types::ISOLATION_DOMAIN) {
            apps_csp(state, false)
        } else {
            shell_csp(state)
        },
    );
    if framed {
        headers.remove(header::X_FRAME_OPTIONS);
    } else {
        headers.insert(header::X_FRAME_OPTIONS, HeaderValue::from_static("SAMEORIGIN"));
    }
    response
}

async fn shell(State(state): State<AppState>, req: Request) -> Response {
    let path = req.uri().path();
    if path.starts_with("/api/") || path.starts_with("/dav") || path.starts_with("/jmap") || path.starts_with("/caldav") {
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
    response.headers_mut().insert("content-security-policy", shell_csp(&state));
    response
}

fn csp_base() -> &'static str {
    "default-src 'self'; script-src 'self' 'unsafe-inline' 'unsafe-eval' 'wasm-unsafe-eval'; style-src 'self' 'unsafe-inline'; img-src 'self' blob: data:; font-src 'self' data:; connect-src 'self'; worker-src 'self' blob:; object-src 'none'; base-uri 'self'"
}

fn shell_csp(state: &AppState) -> HeaderValue {
    let frame = match state.config.apps_origin() {
        Some(origin) => format!("'self' {origin}"),
        None => "'self'".into(),
    };
    HeaderValue::from_str(&format!("{}; frame-src {frame}", csp_base())).unwrap_or_else(|_| HeaderValue::from_static("default-src 'self'; frame-src 'self'; object-src 'none'"))
}

fn apps_csp(state: &AppState, sandboxed: bool) -> HeaderValue {
    // sandbox without allow-same-origin: the document gets an opaque origin, including
    // when someone opens /apps/<id> directly (not only inside the shell iframe).
    // Separate-domain apps keep a real origin (storage works) and skip the sandbox token.
    let prefix = if sandboxed { "sandbox allow-scripts; " } else { "" };
    HeaderValue::from_str(&format!(
        "{prefix}{}; frame-ancestors {}",
        csp_base(),
        frame_ancestors(state)
    ))
    .unwrap_or_else(|_| HeaderValue::from_static("default-src 'self'; frame-ancestors 'self'; object-src 'none'; connect-src 'self'"))
}

fn parent_origin(state: &AppState, headers: &axum::http::HeaderMap) -> Option<String> {
    if let Some(origin) = state.config.public_origin() {
        return Some(origin);
    }
    let host = headers.get(header::HOST).and_then(|v| v.to_str().ok())?;
    if host.is_empty() || state.config.is_apps_host(host) {
        return None;
    }
    let scheme = if state.config.cookie_is_secure() { "https" } else { "http" };
    Some(format!("{scheme}://{host}"))
}

fn frame_ancestors(state: &AppState) -> String {
    let port = state.config.listen.port();
    let mut allowed = vec!["'self'".to_string(), format!("http://localhost:{port}"), format!("http://127.0.0.1:{port}")];
    if let Some(origin) = state.config.public_origin() {
        allowed.push(origin);
    }
    allowed.dedup();
    allowed.join(" ")
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

