use artcraft_cloud_types::{
    AppStatus, DirListing, ErrorBody, FileEntry, Share, TrashEntry, User, WriteResult,
};
use wasm_bindgen::JsCast;
use wasm_bindgen_futures::JsFuture;
use web_sys::{Request, RequestCredentials, RequestInit, Response};

pub fn enc(value: &str) -> String {
    format!("{}", js_sys::encode_uri_component(value))
}

pub struct Reply {
    pub status: u16,
    pub body: Vec<u8>,
    pub etag: Option<String>,
}

pub async fn send(
    method: &str,
    url: &str,
    body: Option<&[u8]>,
    content_type: Option<&str>,
    if_match: Option<&str>,
) -> Result<Reply, String> {
    let window = web_sys::window().ok_or("no window")?;
    let init = RequestInit::new();
    init.set_method(method);
    init.set_credentials(RequestCredentials::Include);
    if let Some(body) = body {
        let bytes = js_sys::Uint8Array::from(body);
        init.set_body(&bytes);
    }
    let request = Request::new_with_str_and_init(url, &init).map_err(|err| format!("{err:?}"))?;
    request
        .headers()
        .set("x-requested-with", "artcraft")
        .map_err(|err| format!("{err:?}"))?;
    if let Some(content_type) = content_type {
        request
            .headers()
            .set("content-type", content_type)
            .map_err(|err| format!("{err:?}"))?;
    }
    if let Some(if_match) = if_match {
        request
            .headers()
            .set("if-match", if_match)
            .map_err(|err| format!("{err:?}"))?;
    }
    let response = JsFuture::from(window.fetch_with_request(&request))
        .await
        .map_err(|err| format!("{err:?}"))?;
    let response: Response = response
        .dyn_into()
        .map_err(|_| "not a response".to_string())?;
    let status = response.status();
    let etag = response
        .headers()
        .get("etag")
        .ok()
        .flatten()
        .map(|value| value.trim_matches('"').to_string());
    let buffer = JsFuture::from(response.array_buffer().map_err(|err| format!("{err:?}"))?)
        .await
        .map_err(|err| format!("{err:?}"))?;
    let bytes = js_sys::Uint8Array::new(&buffer).to_vec();
    Ok(Reply {
        status,
        body: bytes,
        etag,
    })
}

pub async fn get_json<T: serde::de::DeserializeOwned>(url: &str) -> Result<T, String> {
    let reply = send("GET", url, None, None, None).await?;
    decode(reply)
}

pub async fn post_json<T: serde::Serialize, R: serde::de::DeserializeOwned>(
    url: &str,
    body: &T,
) -> Result<R, String> {
    let bytes = serde_json::to_vec(body).map_err(|err| err.to_string())?;
    let reply = send("POST", url, Some(&bytes), Some("application/json"), None).await?;
    decode(reply)
}

pub async fn post_empty(url: &str) -> Result<(), String> {
    let reply = send("POST", url, Some(b"{}"), Some("application/json"), None).await?;
    if reply.status >= 400 {
        Err(message(&reply.body))
    } else {
        Ok(())
    }
}

fn decode<T: serde::de::DeserializeOwned>(reply: Reply) -> Result<T, String> {
    if reply.status == 401 {
        return Err("unauthorized".into());
    }
    if reply.status >= 400 {
        return Err(message(&reply.body));
    }
    if reply.body.is_empty() {
        return serde_json::from_str("null").map_err(|err| err.to_string());
    }
    serde_json::from_slice(&reply.body).map_err(|err| err.to_string())
}

fn message(body: &[u8]) -> String {
    serde_json::from_slice::<ErrorBody>(body)
        .map(|err| err.error)
        .unwrap_or_else(|_| String::from_utf8_lossy(body).to_string())
}

pub async fn me() -> Result<User, String> {
    get_json("/api/me").await
}

pub async fn login(username: &str, password: &str) -> Result<User, String> {
    post_json(
        "/api/session",
        &serde_json::json!({ "username": username, "password": password }),
    )
    .await
}

pub async fn logout() -> Result<(), String> {
    let reply = send("DELETE", "/api/session", None, None, None).await?;
    if reply.status >= 400 {
        Err(message(&reply.body))
    } else {
        Ok(())
    }
}

pub async fn list_dir(path: &str) -> Result<DirListing, String> {
    get_json(&format!("/api/files?path={}", enc(path))).await
}

pub async fn apps() -> Result<Vec<AppStatus>, String> {
    get_json("/api/apps").await
}

pub async fn trash() -> Result<Vec<TrashEntry>, String> {
    get_json("/api/trash").await
}

pub async fn shares(incoming: bool) -> Result<Vec<Share>, String> {
    let which = if incoming { "in" } else { "out" };
    get_json(&format!("/api/shares?box={which}")).await
}

pub enum PutFail {
    Conflict(Option<String>),
    Other(String),
}

pub async fn put_bytes(
    url: &str,
    bytes: &[u8],
    if_match: Option<&str>,
) -> Result<WriteResult, PutFail> {
    let reply = send(
        "PUT",
        url,
        Some(bytes),
        Some("application/octet-stream"),
        if_match,
    )
    .await
    .map_err(PutFail::Other)?;
    if reply.status == 409 {
        let etag = serde_json::from_slice::<ErrorBody>(&reply.body)
            .ok()
            .and_then(|err| err.etag);
        return Err(PutFail::Conflict(etag));
    }
    if reply.status >= 400 {
        return Err(PutFail::Other(message(&reply.body)));
    }
    serde_json::from_slice(&reply.body).map_err(|err| PutFail::Other(err.to_string()))
}

pub fn home_content(path: &str) -> String {
    format!("/api/files/content?path={}", enc(path))
}

pub fn share_content(id: i64, path: &str) -> String {
    format!("/api/shares/{id}/content?path={}", enc(path))
}

pub fn public_content(token: &str, path: &str) -> String {
    format!("/api/public/{token}/content?path={}", enc(path))
}

pub async fn upload(dir: &str, name: &str, bytes: &[u8]) -> Result<FileEntry, String> {
    let window = web_sys::window().ok_or("no window")?;
    let form = web_sys::FormData::new().map_err(|err| format!("{err:?}"))?;
    let blob = web_sys::Blob::new_with_u8_array_sequence(&js_sys::Array::of1(
        &js_sys::Uint8Array::from(bytes),
    ))
    .map_err(|err| format!("{err:?}"))?;
    form.append_with_blob_and_filename("file", &blob, name)
        .map_err(|err| format!("{err:?}"))?;
    let init = RequestInit::new();
    init.set_method("POST");
    init.set_credentials(RequestCredentials::Include);
    init.set_body(&form);
    let url = format!("/api/files/upload?path={}", enc(dir));
    let request = Request::new_with_str_and_init(&url, &init).map_err(|err| format!("{err:?}"))?;
    request
        .headers()
        .set("x-requested-with", "artcraft")
        .map_err(|err| format!("{err:?}"))?;
    let response = JsFuture::from(window.fetch_with_request(&request))
        .await
        .map_err(|err| format!("{err:?}"))?;
    let response: Response = response
        .dyn_into()
        .map_err(|_| "not a response".to_string())?;
    let status = response.status();
    let buffer = JsFuture::from(response.array_buffer().map_err(|err| format!("{err:?}"))?)
        .await
        .map_err(|err| format!("{err:?}"))?;
    let body = js_sys::Uint8Array::new(&buffer).to_vec();
    if status >= 400 {
        return Err(message(&body));
    }
    serde_json::from_slice(&body).map_err(|err| err.to_string())
}

/// Any request whose reply body does not matter: `Err` carries the server's message.
pub async fn call(method: &str, url: &str, body: Option<&serde_json::Value>) -> Result<(), String> {
    let bytes = body.map(|value| serde_json::to_vec(value).unwrap_or_default());
    let reply = send(
        method,
        url,
        bytes.as_deref(),
        bytes.as_ref().map(|_| "application/json"),
        None,
    )
    .await?;
    if reply.status >= 400 {
        Err(message(&reply.body))
    } else {
        Ok(())
    }
}

pub async fn delete(url: &str) -> Result<(), String> {
    call("DELETE", url, None).await
}

/// The settings object the server keeps for this user and app (`{}` when there is none).
pub async fn settings(app: &str) -> Result<serde_json::Map<String, serde_json::Value>, String> {
    get_json(&format!("/api/settings/{app}")).await
}

pub async fn put_settings(
    app: &str,
    value: &serde_json::Map<String, serde_json::Value>,
) -> Result<(), String> {
    let bytes = serde_json::to_vec(value).map_err(|err| err.to_string())?;
    let reply = send(
        "PUT",
        &format!("/api/settings/{app}"),
        Some(&bytes),
        Some("application/json"),
        None,
    )
    .await?;
    if reply.status >= 400 {
        Err(message(&reply.body))
    } else {
        Ok(())
    }
}

pub async fn directory() -> Result<artcraft_cloud_types::Directory, String> {
    get_json("/api/directory").await
}

pub async fn file_kind(path: &str) -> Result<artcraft_cloud_types::FileKind, String> {
    get_json(&format!("/api/files/kind?path={}", enc(path))).await
}

/// [`call`] with owned arguments, for futures that outlive the caller.
pub fn req(
    method: &'static str,
    url: impl Into<String>,
    body: Option<serde_json::Value>,
) -> impl std::future::Future<Output = Result<(), String>> + 'static {
    let url = url.into();
    async move { call(method, &url, body.as_ref()).await }
}

pub async fn preferences() -> Result<artcraft_cloud_types::prefs::Preferences, String> {
    get_json("/api/preferences").await
}

pub async fn put_preferences(
    prefs: &artcraft_cloud_types::prefs::Preferences,
) -> Result<artcraft_cloud_types::prefs::Preferences, String> {
    let bytes = serde_json::to_vec(prefs).map_err(|err| err.to_string())?;
    let reply = send("PUT", "/api/preferences", Some(&bytes), Some("application/json"), None).await?;
    decode(reply)
}
