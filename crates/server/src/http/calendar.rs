//! Calendars: what CalendarCraft needs to show a user's calendars.
//!
//! - `/caldav/*` is the signed-in user's CalDAV space on the mail server (Stalwart keeps
//!   calendars next to mail, at `/dav/cal/<account>/`). The session cookie names the user; the
//!   request goes on with the mailbox's own app password (the one the JMAP proxy uses), which
//!   never reaches the browser. Paths in the replies are rewritten from `/dav/cal/` to `/caldav/`
//!   so they resolve on this origin (this server's own WebDAV drive is `/dav/`).
//! - `/api/calendar/remote` passes a request on to another calendar server (an iCloud or Fastmail
//!   CalDAV account, a public `.ics` feed) that the browser cannot reach itself: the shell's
//!   content security policy keeps apps on this origin, and most servers send no CORS headers.
//!   The target is `x-calendar-url`, the credential `x-calendar-auth`. Only http(s), only public
//!   addresses (unless `ARTCRAFT_CALENDAR_ALLOW_PRIVATE=1`), the address checked is the one
//!   connected to, and redirects come back to the app instead of being followed.
//! - `/api/calendar/calendarcraft.js` is `window.calendarcraftHost` for the signed-in user,
//!   loaded by CalendarCraft's page before the app starts (as MailCraft's hook).

use std::net::{IpAddr, SocketAddr};
use std::time::Duration;

use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::{header, HeaderMap, HeaderName, HeaderValue, Method, StatusCode};
use axum::response::{IntoResponse, Response};

use super::mail::{bad_gateway, mailbox_context, not_configured};
use super::session::AuthUser;
use crate::error::ApiError;
use crate::state::AppState;
use crate::vault;

/// Largest request body passed on (an event with attachments is far smaller).
const REQUEST_LIMIT: usize = 8 * 1024 * 1024;
/// Largest reply passed on (a busy calendar's REPORT for a year, a big public feed).
const REPLY_LIMIT: usize = 32 * 1024 * 1024;
const LOCAL_PREFIX: &str = "/caldav";
const UPSTREAM_PREFIX: &str = "/dav/cal";

fn methods() -> [Method; 7] {
    let dav = |m: &str| Method::from_bytes(m.as_bytes()).expect("method");
    [Method::GET, Method::HEAD, Method::OPTIONS, Method::PUT, Method::DELETE, dav("PROPFIND"), dav("REPORT")]
}

fn allowed(method: &Method) -> bool {
    methods().contains(method)
}

/// Asked for by CalendarCraft's own requests; a page elsewhere cannot add it without CORS.
fn from_the_app(headers: &HeaderMap) -> bool {
    headers.get("x-requested-with").and_then(|v| v.to_str().ok()) == Some("artcraft")
}

fn forbidden() -> Response {
    ApiError::new(StatusCode::FORBIDDEN, "missing X-Requested-With: artcraft").into_response()
}

/// Request headers a calendar server needs.
const FORWARD: [&str; 7] = ["content-type", "depth", "if-match", "if-none-match", "accept", "prefer", "brief"];
/// Reply headers the app needs.
const RETURN: [&str; 6] = ["content-type", "etag", "location", "dav", "allow", "last-modified"];

fn copy_reply_headers(reply: &reqwest::header::HeaderMap, rewrite_location: impl Fn(&str) -> String) -> HeaderMap {
    let mut out = HeaderMap::new();
    for name in RETURN {
        let Some(value) = reply.get(name) else { continue };
        let value = if name == "location" {
            value.to_str().ok().map(&rewrite_location).and_then(|v| HeaderValue::from_str(&v).ok())
        } else {
            HeaderValue::from_bytes(value.as_bytes()).ok()
        };
        if let Some(value) = value {
            out.insert(HeaderName::from_static(name), value);
        }
    }
    out.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    out
}

#[allow(clippy::result_large_err)]
async fn read_limited(reply: reqwest::Response) -> Result<Vec<u8>, Response> {
    if reply.content_length().is_some_and(|len| len as usize > REPLY_LIMIT) {
        return Err(bad_gateway("the calendar server's reply is too large").into_response());
    }
    let mut reply = reply;
    let mut body = Vec::new();
    while let Some(chunk) = reply.chunk().await.map_err(|_| bad_gateway("the calendar server's reply was cut off").into_response())? {
        if body.len() + chunk.len() > REPLY_LIMIT {
            return Err(bad_gateway("the calendar server's reply is too large").into_response());
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

/// Only the CalDAV space, never a path that climbs out of it upstream.
fn caldav_path(path: &str) -> Option<&str> {
    let rest = path.strip_prefix(LOCAL_PREFIX)?;
    let ok = (rest.is_empty() || rest.starts_with('/'))
        && !rest.split('/').any(|segment| segment == ".." || segment == ".")
        && !rest.to_ascii_lowercase().contains("%2e")
        && !rest.to_ascii_lowercase().contains("%2f")
        && !rest.contains('\\');
    ok.then_some(rest)
}

/// Upstream paths and URLs in a reply, as paths on this origin.
pub fn rewrite_to_local(text: &str, upstream_base: &str) -> String {
    let base = upstream_base.trim_end_matches('/');
    let text = if base.is_empty() { text.to_string() } else { text.replace(&format!("{base}{UPSTREAM_PREFIX}"), UPSTREAM_PREFIX) };
    text.replace(&format!("{UPSTREAM_PREFIX}/"), &format!("{LOCAL_PREFIX}/"))
}

/// `/caldav/*`: the signed-in user's calendars on the mail server.
pub async fn caldav_proxy(State(state): State<AppState>, AuthUser(user): AuthUser, req: Request) -> Response {
    match caldav(&state, user, req).await {
        Ok(response) | Err(response) => response,
    }
}

#[allow(clippy::result_large_err)]
async fn caldav(state: &AppState, user: crate::db::Account, req: Request) -> Result<Response, Response> {
    if !from_the_app(req.headers()) {
        return Err(forbidden());
    }
    if !allowed(req.method()) {
        return Err(StatusCode::METHOD_NOT_ALLOWED.into_response());
    }
    let Some(rest) = caldav_path(req.uri().path()).map(str::to_string) else {
        return Err(ApiError::new(StatusCode::BAD_REQUEST, "bad path").into_response());
    };
    let mailbox = state.db.mailbox(user.id).map_err(|err| ApiError::from(err).into_response())?.ok_or_else(|| ApiError::new(StatusCode::NOT_FOUND, "no mailbox is linked, so there is no calendar").into_response())?;
    let config = state.db.mail_config().map_err(|err| ApiError::from(err).into_response())?.ok_or_else(|| not_configured().into_response())?;
    let password = vault::open(&state.secret, &mailbox_context(user.id), &mailbox.secret).map_err(|err| ApiError::from(err).into_response())?;
    let base = config.base_url.trim_end_matches('/').to_string();
    let query = req.uri().query().map(|q| format!("?{q}")).unwrap_or_default();
    let method = req.method().clone();
    let headers = req.headers().clone();
    let body = axum::body::to_bytes(req.into_body(), REQUEST_LIMIT).await.map_err(|_| ApiError::new(StatusCode::PAYLOAD_TOO_LARGE, "request is too large").into_response())?;
    let mut upstream = crate::stalwart::http()
        .request(method.clone(), format!("{base}{UPSTREAM_PREFIX}{rest}{query}"))
        .basic_auth(&mailbox.address, Some(&password))
        .timeout(Duration::from_secs(60));
    for name in FORWARD {
        if let Some(value) = headers.get(name) {
            upstream = upstream.header(name, value.as_bytes());
        }
    }
    if !body.is_empty() {
        upstream = upstream.body(body);
    }
    let reply = upstream.send().await.map_err(|_| bad_gateway("the calendar server could not be reached").into_response())?;
    let status = StatusCode::from_u16(reply.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
    if status == StatusCode::UNAUTHORIZED {
        tracing::warn!(user = %user.username, "the mail server refused a linked mailbox's password for CalDAV");
        return Err(bad_gateway("the calendar server refused this account").into_response());
    }
    let out = copy_reply_headers(reply.headers(), |location| rewrite_to_local(location, &base));
    let xml = out.get(header::CONTENT_TYPE).and_then(|v| v.to_str().ok()).is_some_and(|t| t.contains("xml"));
    let bytes = read_limited(reply).await?;
    let bytes = if xml { rewrite_to_local(&String::from_utf8_lossy(&bytes), &base).into_bytes() } else { bytes };
    let mut response = Response::new(Body::from(bytes));
    *response.status_mut() = status;
    *response.headers_mut() = out;
    Ok(response)
}

/// Addresses the remote proxy never connects to: this machine, the private networks, the
/// link-local range (cloud metadata), multicast and other reserved space.
pub fn is_public(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            let [a, b, ..] = v4.octets();
            !(v4.is_private()
                || v4.is_loopback()
                || v4.is_link_local()
                || v4.is_unspecified()
                || v4.is_broadcast()
                || v4.is_multicast()
                || v4.is_documentation()
                || a == 0
                || (a == 100 && (64..128).contains(&b))
                || (a == 192 && b == 0)
                || (a == 198 && (b == 18 || b == 19))
                || a >= 240)
        }
        IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                return is_public(IpAddr::V4(v4));
            }
            let first = v6.segments()[0];
            !(v6.is_loopback() || v6.is_unspecified() || v6.is_multicast() || (first & 0xfe00) == 0xfc00 || (first & 0xffc0) == 0xfe80 || first == 0x2001 && v6.segments()[1] == 0x0db8 || first == 0x0064)
        }
    }
}

fn private_allowed() -> bool {
    std::env::var("ARTCRAFT_CALENDAR_ALLOW_PRIVATE").is_ok_and(|v| v == "1" || v.eq_ignore_ascii_case("true"))
}

fn remote_error(status: StatusCode, message: &str) -> Response {
    ApiError::new(status, message).into_response()
}

/// `/api/calendar/remote`: one request to another calendar server, for CalendarCraft.
pub async fn remote_proxy(State(_state): State<AppState>, AuthUser(user): AuthUser, req: Request) -> Response {
    match remote(&user, req).await {
        Ok(response) | Err(response) => response,
    }
}

#[allow(clippy::result_large_err)]
async fn remote(user: &crate::db::Account, req: Request) -> Result<Response, Response> {
    if !from_the_app(req.headers()) {
        return Err(forbidden());
    }
    if !allowed(req.method()) {
        return Err(StatusCode::METHOD_NOT_ALLOWED.into_response());
    }
    let target = req.headers().get("x-calendar-url").and_then(|v| v.to_str().ok()).unwrap_or("").trim().to_string();
    let url = url::Url::parse(&target).map_err(|_| remote_error(StatusCode::BAD_REQUEST, "x-calendar-url is not a URL"))?;
    if !matches!(url.scheme(), "https" | "http") || !url.username().is_empty() || url.password().is_some() {
        return Err(remote_error(StatusCode::BAD_REQUEST, "only http and https calendar addresses, without a user name in them"));
    }
    let host = url.host_str().ok_or_else(|| remote_error(StatusCode::BAD_REQUEST, "the address has no host"))?.to_string();
    let port = url.port_or_known_default().unwrap_or(443);
    let lookup_host = host.trim_start_matches('[').trim_end_matches(']').to_string();
    let addrs: Vec<SocketAddr> = tokio::net::lookup_host((lookup_host.as_str(), port)).await.map_err(|_| remote_error(StatusCode::BAD_GATEWAY, "the calendar server's name does not resolve"))?.collect();
    let allow_private = private_allowed();
    let Some(addr) = addrs.iter().copied().find(|a| allow_private || is_public(a.ip())) else {
        return Err(remote_error(StatusCode::FORBIDDEN, "that address is on a private network"));
    };
    // Connect to the address that was checked, not to whatever a second lookup returns.
    let client = reqwest::Client::builder()
        .user_agent(concat!("artcraft-cloud/", env!("CARGO_PKG_VERSION"), " (CalendarCraft)"))
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(8))
        .resolve(&lookup_host, addr)
        .build()
        .map_err(|_| remote_error(StatusCode::INTERNAL_SERVER_ERROR, "http client"))?;
    let method = req.method().clone();
    let headers = req.headers().clone();
    let body = axum::body::to_bytes(req.into_body(), REQUEST_LIMIT).await.map_err(|_| remote_error(StatusCode::PAYLOAD_TOO_LARGE, "request is too large"))?;
    let mut upstream = client.request(method, url.clone()).timeout(Duration::from_secs(60));
    for name in FORWARD {
        if let Some(value) = headers.get(name) {
            upstream = upstream.header(name, value.as_bytes());
        }
    }
    if let Some(auth) = headers.get("x-calendar-auth") {
        upstream = upstream.header(header::AUTHORIZATION, auth.as_bytes());
    }
    if !body.is_empty() {
        upstream = upstream.body(body);
    }
    let reply = upstream.send().await.map_err(|_| remote_error(StatusCode::BAD_GATEWAY, "the calendar server could not be reached"))?;
    let status = StatusCode::from_u16(reply.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
    tracing::debug!(user = %user.username, host = %host, status = status.as_u16(), "calendar request passed on");
    // WWW-Authenticate is not passed on: the browser would ask for a password itself.
    let out = copy_reply_headers(reply.headers(), str::to_string);
    let bytes = read_limited(reply).await?;
    let mut response = Response::new(Body::from(bytes));
    *response.status_mut() = status;
    *response.headers_mut() = out;
    Ok(response)
}

/// `GET /api/calendar/calendarcraft.js`: `window.calendarcraftHost` for the signed-in user.
pub async fn calendarcraft_hook(State(state): State<AppState>, AuthUser(user): AuthUser) -> Result<Response, ApiError> {
    let mailbox = state.db.mailbox(user.id).map_err(ApiError::from)?;
    let configured = state.db.mail_config().map_err(ApiError::from)?.is_some();
    let accounts = match mailbox.filter(|_| configured) {
        Some(mailbox) => serde_json::json!([{
            "id": "artcraft",
            "name": "artcraftCloud",
            "email": mailbox.address,
            "url": format!("{LOCAL_PREFIX}/{}/", urlencoding(&mailbox.address)),
            "provider": "artcraftCloud",
        }]),
        None => serde_json::json!([]),
    };
    let host = serde_json::json!({
        "accounts": accounts,
        "remoteProxy": "/api/calendar/remote",
        "user": { "name": user.display_name },
    });
    let script = format!("// artcraftCloud: the signed-in user's calendars, through this server.\nwindow.calendarcraftHost = {host};\n");
    Ok(([(header::CONTENT_TYPE, "text/javascript; charset=utf-8"), (header::CACHE_CONTROL, "no-store")], script).into_response())
}

/// An email address as one path segment (`@` and anything unusual escaped).
fn urlencoding(text: &str) -> String {
    let mut out = String::new();
    for b in text.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// CalendarCraft's `index.html` with the hook script in front of the app's own scripts.
pub fn inject_hook(html: &str) -> String {
    super::mail::inject_script(html, "/api/calendar/calendarcraft.js")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_proxy_stays_in_the_caldav_space() {
        assert_eq!(caldav_path("/caldav"), Some(""));
        assert_eq!(caldav_path("/caldav/ada%40example.org/default/x.ics"), Some("/ada%40example.org/default/x.ics"));
        for bad in ["/caldav/../jmap", "/caldav/./x", "/caldav/%2e%2e/x", "/caldav/a%2F..%2Fb", "/caldavx", "/caldav/a\\b", "/dav/cal/x"] {
            assert_eq!(caldav_path(bad), None, "{bad}");
        }
    }

    #[test]
    fn upstream_paths_become_local() {
        let xml = "<d:href>/dav/cal/ada%40x.org/default/</d:href><d:href>http://stalwart:8080/dav/cal/ada%40x.org/</d:href><d:href>/dav/pal/ada/</d:href>";
        assert_eq!(
            rewrite_to_local(xml, "http://stalwart:8080/"),
            "<d:href>/caldav/ada%40x.org/default/</d:href><d:href>/caldav/ada%40x.org/</d:href><d:href>/dav/pal/ada/</d:href>"
        );
    }

    #[test]
    fn private_addresses_are_refused() {
        for ip in ["127.0.0.1", "10.1.2.3", "172.16.0.1", "192.168.1.1", "169.254.169.254", "100.64.0.1", "0.0.0.0", "::1", "fd00::1", "fe80::1", "::ffff:127.0.0.1", "224.0.0.1"] {
            assert!(!is_public(ip.parse().unwrap()), "{ip}");
        }
        for ip in ["17.253.144.10", "1.1.1.1", "2606:4700::1111"] {
            assert!(is_public(ip.parse().unwrap()), "{ip}");
        }
    }

    #[test]
    fn addresses_are_one_segment() {
        assert_eq!(urlencoding("ada.l@example.org"), "ada.l%40example.org");
        assert_eq!(urlencoding("a/b"), "a%2Fb");
    }

    #[test]
    fn the_hook_goes_first_in_head() {
        let out = inject_hook("<html><head><script src=app.js></script></head></html>");
        assert!(out.starts_with("<html><head><script src=\"/api/calendar/calendarcraft.js\"></script><script src=app.js>"));
    }

    #[test]
    fn only_calendar_methods() {
        assert!(allowed(&Method::from_bytes(b"REPORT").unwrap()));
        assert!(allowed(&Method::from_bytes(b"PROPFIND").unwrap()));
        assert!(!allowed(&Method::from_bytes(b"PROPPATCH").unwrap()));
        assert!(!allowed(&Method::from_bytes(b"MOVE").unwrap()));
        assert!(!allowed(&Method::POST));
    }
}
