use std::path::{Component, Path, PathBuf};

use axum::body::Body;
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use tokio_util::io::ReaderStream;

pub fn safe_join(root: &Path, rel: &str) -> Option<PathBuf> {
    let mut out = root.to_path_buf();
    if rel.is_empty() {
        return Some(out);
    }
    for component in Path::new(rel).components() {
        match component {
            Component::Normal(part) => out.push(part),
            Component::CurDir => {}
            _ => return None,
        }
    }
    Some(out)
}

pub async fn serve_path(root: &Path, rel: &str, headers: &HeaderMap, cache: CachePolicy) -> Response {
    let Some(mut path) = safe_join(root, rel.trim_start_matches('/')) else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    if path.is_dir() {
        path.push("index.html");
    }
    let Ok(meta) = std::fs::symlink_metadata(&path) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    if meta.file_type().is_symlink() || !meta.is_file() {
        return StatusCode::NOT_FOUND.into_response();
    }
    let Ok(real_root) = root.canonicalize() else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let Ok(real) = path.canonicalize() else {
        return StatusCode::NOT_FOUND.into_response();
    };
    if !real.starts_with(&real_root) {
        return StatusCode::BAD_REQUEST.into_response();
    }
    let accept = headers.get(header::ACCEPT_ENCODING).and_then(|v| v.to_str().ok()).unwrap_or("");
    let (file_path, encoding) = negotiated(&path, accept);
    let Ok(file) = tokio::fs::File::open(&file_path).await else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let len = file.metadata().await.map(|m| m.len()).unwrap_or(0);
    let stream = ReaderStream::new(file);
    let body = Body::from_stream(stream);
    let mut response = Response::new(body);
    let headers = response.headers_mut();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static(content_type(&path)));
    headers.insert(header::CONTENT_LENGTH, HeaderValue::from_str(&len.to_string()).unwrap_or(HeaderValue::from_static("0")));
    headers.insert(header::ACCEPT_RANGES, HeaderValue::from_static("none"));
    headers.insert(header::VARY, HeaderValue::from_static("Accept-Encoding"));
    headers.insert(header::X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
    if let Some(encoding) = encoding {
        headers.insert(header::CONTENT_ENCODING, HeaderValue::from_static(encoding));
    }
    let cache_value = match cache {
        CachePolicy::Immutable if is_hashed(&path) => "public, max-age=31536000, immutable",
        CachePolicy::Html | CachePolicy::Immutable if is_html(&path) => "no-cache",
        _ => "public, max-age=3600",
    };
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static(cache_value));
    let etag = format!("\"{:x}-{:x}\"", len, meta.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_secs()).unwrap_or(0));
    if let Ok(value) = HeaderValue::from_str(&etag) {
        headers.insert(header::ETAG, value);
    }
    response
}

#[derive(Clone, Copy)]
pub enum CachePolicy {
    Immutable,
    Html,
}

fn negotiated(path: &Path, accept: &str) -> (PathBuf, Option<&'static str>) {
    if accept.contains("br") {
        let candidate = append(path, ".br");
        if candidate.is_file() {
            return (candidate, Some("br"));
        }
    }
    if accept.contains("gzip") {
        let candidate = append(path, ".gz");
        if candidate.is_file() {
            return (candidate, Some("gzip"));
        }
    }
    (path.to_path_buf(), None)
}

fn append(path: &Path, suffix: &str) -> PathBuf {
    let mut os = path.as_os_str().to_os_string();
    os.push(suffix);
    PathBuf::from(os)
}

fn is_html(path: &Path) -> bool {
    path.extension().and_then(|e| e.to_str()) == Some("html")
}

fn is_hashed(path: &Path) -> bool {
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
    name.split(['-', '.']).any(|part| part.len() >= 8 && part.chars().all(|c| c.is_ascii_hexdigit()))
}

pub fn content_type(path: &Path) -> &'static str {
    match path.extension().and_then(|ext| ext.to_str()).unwrap_or("") {
        "html" => "text/html; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "wasm" => "application/wasm",
        "json" => "application/json",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "ico" => "image/x-icon",
        "txt" | "md" => "text/plain; charset=utf-8",
        "pdf" => "application/pdf",
        "woff2" => "font/woff2",
        _ => "application/octet-stream",
    }
}

pub fn attachment_disposition(name: &str) -> String {
    let fallback: String = name.chars().map(|c| if c.is_ascii_alphanumeric() || "._- ".contains(c) { c } else { '_' }).collect();
    let encoded = percent_encoding::utf8_percent_encode(name, percent_encoding::NON_ALPHANUMERIC);
    format!("attachment; filename=\"{fallback}\"; filename*=UTF-8''{encoded}")
}
