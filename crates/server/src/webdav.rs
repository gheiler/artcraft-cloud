use std::path::Path;

use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::{header, HeaderMap, HeaderValue, Method, StatusCode};
use axum::response::{IntoResponse, Response};
use base64::Engine;
use percent_encoding::{percent_decode_str, utf8_percent_encode, AsciiSet, CONTROLS};

use crate::db::Account;
use crate::error::{ApiError, Error};
use crate::password::verify_password;
use crate::paths::{quote_etag, random_id, sha256_hex, RelPath};
use crate::state::AppState;
use crate::static_files::content_type;
use artcraft_cloud_types::FileEntry;

const PATH_ESCAPE: &AsciiSet = &CONTROLS.add(b' ').add(b'"').add(b'#').add(b'%').add(b'?').add(b'<').add(b'>');

pub async fn dav(State(state): State<AppState>, req: Request) -> Response {
    let (parts, body) = req.into_parts();
    let user = match basic_user(&state, &parts.headers) {
        Ok(user) => user,
        Err(response) => return *response,
    };
    if let Err(err) = state.store.ensure_home(user.id) {
        return ApiError::from(err).into_response();
    }
    let rel = match dav_rel(parts.uri.path()) {
        Ok(rel) => rel,
        Err(err) => return ApiError::from(err).into_response(),
    };
    let method = parts.method.clone();
    let result = if method == Method::OPTIONS {
        Ok(options())
    } else if method == Method::from_bytes(b"PROPFIND").unwrap_or(Method::GET) && method.as_str() == "PROPFIND" {
        propfind(&state, user.id, &rel, &parts.headers)
    } else if method == Method::GET || method == Method::HEAD {
        get_file(&state, user.id, &rel, method == Method::HEAD).await
    } else if method == Method::PUT {
        put_file(&state, user.id, &rel, &parts.headers, body).await
    } else if method == Method::DELETE {
        delete_path(&state, user.id, &rel)
    } else if method.as_str() == "MKCOL" {
        mkcol(&state, user.id, &rel)
    } else if method.as_str() == "MOVE" || method.as_str() == "COPY" {
        transfer(&state, user.id, &rel, &parts.headers, method.as_str() == "COPY")
    } else {
        Err(Error::Invalid("method not allowed".into()))
    };
    match result {
        Ok(response) => response,
        Err(Error::NotFound) if method.as_str() == "MKCOL" => StatusCode::CONFLICT.into_response(),
        Err(err) => ApiError::from(err).into_response(),
    }
}

fn basic_user(state: &AppState, headers: &HeaderMap) -> Result<Account, Box<Response>> {
    let Some(value) = headers.get(header::AUTHORIZATION).and_then(|v| v.to_str().ok()) else {
        return Err(unauthorized());
    };
    let Some(encoded) = value.strip_prefix("Basic ") else {
        return Err(unauthorized());
    };
    let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(encoded.trim()) else {
        return Err(unauthorized());
    };
    let Ok(text) = String::from_utf8(bytes) else {
        return Err(unauthorized());
    };
    let Some((username, password)) = text.split_once(':') else {
        return Err(unauthorized());
    };
    let username = username.trim().to_lowercase();
    if password.len() > 1024 || !state.throttle.allow(&format!("dav:{username}"), 20, 600) {
        return Err(Box::new(StatusCode::TOO_MANY_REQUESTS.into_response()));
    }
    let cache_key = sha256_hex(format!("{username}\0{password}").as_bytes());
    if let Some(id) = state.basic_cache.get(&cache_key) {
        if let Ok(Some(account)) = state.db.user_by_id(id) {
            if !account.disabled {
                return Ok(account);
            }
        }
    }
    let stored = state.db.user_by_username(&username).ok().flatten();
    let hash = stored.as_ref().map(|a| a.password_hash.as_str()).unwrap_or(state.dummy_hash.as_str());
    let matches = verify_password(password, hash);
    let Some(account) = stored else {
        return Err(unauthorized());
    };
    if account.disabled || !matches {
        return Err(unauthorized());
    }
    state.basic_cache.insert(cache_key, account.id, 60);
    Ok(account)
}

fn unauthorized() -> Box<Response> {
    let mut response = StatusCode::UNAUTHORIZED.into_response();
    response.headers_mut().insert(header::WWW_AUTHENTICATE, HeaderValue::from_static("Basic realm=\"artcraftCloud\""));
    Box::new(response)
}

fn options() -> Response {
    let mut response = StatusCode::NO_CONTENT.into_response();
    let headers = response.headers_mut();
    headers.insert("DAV", HeaderValue::from_static("1"));
    headers.insert(header::ALLOW, HeaderValue::from_static("OPTIONS, GET, HEAD, PUT, DELETE, MKCOL, COPY, MOVE, PROPFIND"));
    response
}

fn dav_rel(path: &str) -> Result<RelPath, Error> {
    let decoded = percent_decode_str(path).decode_utf8_lossy();
    let rest = decoded.trim_start_matches("/dav").trim_start_matches('/');
    RelPath::parse(rest)
}

fn propfind(state: &AppState, owner: i64, rel: &RelPath, headers: &HeaderMap) -> Result<Response, Error> {
    let depth = match headers.get("depth").and_then(|v| v.to_str().ok()).unwrap_or("infinity") {
        "0" => 0,
        "1" => 1,
        _ => 8,
    };
    let mut items = Vec::new();
    collect(state, owner, rel, depth, &mut items)?;
    let mut xml = String::from(r#"<?xml version="1.0" encoding="utf-8"?>"#);
    xml.push_str(r#"<d:multistatus xmlns:d="DAV:">"#);
    for (href, entry) in items {
        xml.push_str("<d:response><d:href>");
        xml.push_str(&xml_escape(&href));
        xml.push_str("</d:href><d:propstat><d:prop>");
        xml.push_str("<d:displayname>");
        xml.push_str(&xml_escape(if entry.name.is_empty() { "/" } else { &entry.name }));
        xml.push_str("</d:displayname>");
        if !entry.is_dir {
            xml.push_str(&format!("<d:getcontentlength>{}</d:getcontentlength>", entry.size));
        }
        if !entry.etag.is_empty() {
            xml.push_str(&format!("<d:getetag>{}</d:getetag>", xml_escape(&quote_etag(&entry.etag))));
        }
        xml.push_str("<d:resourcetype>");
        if entry.is_dir {
            xml.push_str("<d:collection/>");
        }
        xml.push_str("</d:resourcetype>");
        let modified = httpdate::fmt_http_date(std::time::UNIX_EPOCH + std::time::Duration::from_secs(entry.modified.max(0) as u64));
        xml.push_str(&format!("<d:getlastmodified>{}</d:getlastmodified>", xml_escape(&modified)));
        xml.push_str("</d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response>");
    }
    xml.push_str("</d:multistatus>");
    let mut response = Response::new(Body::from(xml));
    *response.status_mut() = StatusCode::MULTI_STATUS;
    response.headers_mut().insert(header::CONTENT_TYPE, HeaderValue::from_static("application/xml; charset=utf-8"));
    Ok(response)
}

fn collect(state: &AppState, owner: i64, rel: &RelPath, depth: u32, out: &mut Vec<(String, FileEntry)>) -> Result<(), Error> {
    if out.len() > 10_000 {
        return Err(Error::TooLarge);
    }
    let entry = state.store.stat(owner, rel)?;
    out.push((href(rel, entry.is_dir), entry.clone()));
    if depth > 0 && entry.is_dir {
        for child in state.store.list(owner, rel)? {
            let child_rel = RelPath::parse(&child.path)?;
            if depth == 1 {
                out.push((href(&child_rel, child.is_dir), child));
            } else {
                collect(state, owner, &child_rel, depth - 1, out)?;
            }
        }
    }
    Ok(())
}

fn href(rel: &RelPath, is_dir: bool) -> String {
    let mut out = String::from("/dav/");
    if !rel.is_root() {
        for (index, part) in rel.as_str().split('/').enumerate() {
            if index > 0 {
                out.push('/');
            }
            out.push_str(&utf8_percent_encode(part, PATH_ESCAPE).to_string());
        }
    }
    if is_dir && !out.ends_with('/') {
        out.push('/');
    }
    out
}

async fn get_file(state: &AppState, owner: i64, rel: &RelPath, head: bool) -> Result<Response, Error> {
    let (path, entry) = state.store.open_path(owner, rel)?;
    let mut response = if head {
        Response::new(Body::empty())
    } else {
        let file = tokio::fs::File::open(&path).await?;
        Response::new(Body::from_stream(tokio_util::io::ReaderStream::new(file)))
    };
    let headers = response.headers_mut();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static(content_type(Path::new(&entry.name))));
    headers.insert(header::CONTENT_LENGTH, HeaderValue::from_str(&entry.size.to_string()).unwrap_or(HeaderValue::from_static("0")));
    if let Ok(etag) = HeaderValue::from_str(&quote_etag(&entry.etag)) {
        headers.insert(header::ETAG, etag);
    }
    headers.insert(header::ACCEPT_RANGES, HeaderValue::from_static("none"));
    Ok(response)
}

async fn put_file(state: &AppState, owner: i64, rel: &RelPath, headers: &HeaderMap, body: Body) -> Result<Response, Error> {
    let tmp = state.store.tmp_root.join(format!("dav-{}", random_id()));
    if let Err(err) = write_body(&tmp, body, state.config.max_upload_bytes).await {
        let _ = tokio::fs::remove_file(&tmp).await;
        return Err(err);
    }
    let if_match = headers.get(header::IF_MATCH).and_then(|v| v.to_str().ok()).map(str::to_string);
    // WebDAV PUT overwrites unless If-Match says otherwise. `*` matches any current etag.
    let guard = if_match.or_else(|| Some("*".to_string()));
    let existed = state.store.stat(owner, rel).is_ok();
    let store = state.store.clone();
    let rel = rel.clone();
    let tmp_for_task = tmp.clone();
    let entry = match tokio::task::spawn_blocking(move || store.write_file(owner, &rel, &tmp_for_task, guard.as_deref())).await {
        Ok(result) => result,
        Err(_) => {
            let _ = tokio::fs::remove_file(&tmp).await;
            return Err(Error::Invalid("worker failed".into()));
        }
    };
    let _ = tokio::fs::remove_file(&tmp).await;
    let entry = entry?;
    let status = if existed { StatusCode::NO_CONTENT } else { StatusCode::CREATED };
    let mut response = status.into_response();
    if let Ok(etag) = HeaderValue::from_str(&quote_etag(&entry.etag)) {
        response.headers_mut().insert(header::ETAG, etag);
    }
    Ok(response)
}

pub(crate) async fn write_body(path: &Path, body: Body, max: u64) -> Result<(), Error> {
    let mut file = tokio::fs::File::create(path).await?;
    let mut stream = body.into_data_stream();
    let mut total = 0u64;
    use futures_util::StreamExt;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|err| {
            if err.to_string().to_lowercase().contains("length") {
                Error::TooLarge
            } else {
                Error::Invalid(err.to_string())
            }
        })?;
        total = total.saturating_add(chunk.len() as u64);
        if total > max {
            return Err(Error::TooLarge);
        }
        tokio::io::AsyncWriteExt::write_all(&mut file, &chunk).await?;
    }
    file.sync_all().await?;
    Ok(())
}

fn delete_path(state: &AppState, owner: i64, rel: &RelPath) -> Result<Response, Error> {
    state.store.trash(owner, rel)?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

fn mkcol(state: &AppState, owner: i64, rel: &RelPath) -> Result<Response, Error> {
    state.store.mkdir(owner, rel)?;
    Ok(StatusCode::CREATED.into_response())
}

fn transfer(state: &AppState, owner: i64, from: &RelPath, headers: &HeaderMap, copy: bool) -> Result<Response, Error> {
    let dest_header = headers.get("destination").and_then(|v| v.to_str().ok()).ok_or_else(|| Error::Invalid("missing Destination".into()))?;
    let path = if let Ok(url) = url::Url::parse(dest_header) {
        url.path().to_string()
    } else if dest_header.starts_with('/') {
        dest_header.to_string()
    } else {
        return Err(Error::Invalid("bad Destination".into()));
    };
    let dest = dav_rel(&path)?;
    if dest.as_str() == from.as_str() {
        return Err(Error::Forbidden);
    }
    let overwrite = headers.get("overwrite").and_then(|v| v.to_str().ok()).unwrap_or("T");
    if overwrite.eq_ignore_ascii_case("F") && state.store.stat(owner, &dest).is_ok() {
        return Err(Error::Conflict { etag: String::new() });
    }
    if state.store.stat(owner, &dest).is_ok() {
        state.store.trash(owner, &dest)?;
    }
    let name = dest.name().ok_or(Error::BadPath)?.to_string();
    let parent = dest.parent();
    if copy {
        state.store.copy(owner, from, &parent, Some(&name))?;
    } else {
        state.store.move_into(owner, from, &parent, Some(&name))?;
    }
    Ok(StatusCode::CREATED.into_response())
}

fn xml_escape(value: &str) -> String {
    value.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}
