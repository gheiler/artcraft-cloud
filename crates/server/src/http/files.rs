use axum::extract::{FromRequestParts, Multipart, Path, Query, State};
use axum::http::request::Parts;
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;

use super::blocking;
use super::session::AuthUser;
use crate::error::ApiError;
use crate::paths::{random_id, quote_etag, single_name, RelPath};
use crate::state::AppState;
use crate::static_files::{attachment_disposition, content_type};
use artcraft_cloud_types::{DirListing, FileEntry, PathBody, RenameBody, TransferBody, TrashEntry, WriteResult};

#[derive(Deserialize)]
pub struct PathQuery {
    pub path: Option<String>,
}

fn rel_of(path: Option<&str>) -> Result<RelPath, ApiError> {
    RelPath::parse(path.unwrap_or("")).map_err(ApiError::from)
}

/// The signed-in person, or a capability token when the request comes from an opaque origin.
pub struct FileActor {
    pub user_id: i64,
    pub cap: Option<crate::capability::Capability>,
}

impl FromRequestParts<AppState> for FileActor {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, Self::Rejection> {
        if crate::capability::origin_is_null(&parts.headers) {
            let token = crate::capability::bearer(&parts.headers).ok_or_else(|| {
                ApiError::new(StatusCode::FORBIDDEN, "opaque origin cannot use the session")
            })?;
            let cap = state.capabilities.lookup(token).ok_or_else(|| {
                ApiError::new(StatusCode::FORBIDDEN, "invalid or expired capability token")
            })?;
            return Ok(FileActor { user_id: cap.user_id, cap: Some(cap) });
        }
        let AuthUser(user) = AuthUser::from_request_parts(parts, state).await?;
        Ok(FileActor { user_id: user.id, cap: None })
    }
}

fn cap_allows(actor: &FileActor, path: &str, write: bool) -> Result<(), ApiError> {
    let Some(cap) = actor.cap.as_ref() else {
        return Ok(());
    };
    let ok = if write { cap.can_write(path) } else { cap.can_read(path) };
    if ok {
        Ok(())
    } else {
        Err(ApiError::new(StatusCode::FORBIDDEN, "this capability token does not cover that file"))
    }
}

pub async fn list_files(State(state): State<AppState>, AuthUser(user): AuthUser, Query(query): Query<PathQuery>) -> Result<Json<DirListing>, ApiError> {
    let rel = rel_of(query.path.as_deref())?;
    let id = user.id;
    let store = state.store.clone();
    let entries = blocking(move || store.list(id, &rel)).await?;
    Ok(Json(DirListing { path: query.path.unwrap_or_default(), entries }))
}

pub async fn read_file(State(state): State<AppState>, actor: FileActor, Query(query): Query<PathQuery>) -> Result<Response, ApiError> {
    let rel = rel_of(query.path.as_deref())?;
    cap_allows(&actor, rel.as_str(), false)?;
    let store = state.store.clone();
    let id = actor.user_id;
    let rel2 = rel.clone();
    let (path, entry) = blocking(move || store.open_path(id, &rel2)).await?;
    file_response(path, entry).await
}

pub async fn write_file(
    State(state): State<AppState>,
    actor: FileActor,
    Query(query): Query<PathQuery>,
    headers: axum::http::HeaderMap,
    body: axum::body::Body,
) -> Result<Response, ApiError> {
    let rel = rel_of(query.path.as_deref())?;
    cap_allows(&actor, rel.as_str(), true)?;
    if rel.is_root() {
        return Err(ApiError::from(crate::error::Error::BadPath));
    }
    let tmp = state.store.tmp_root.join(format!("up-{}", random_id()));
    if let Err(err) = crate::webdav::write_body(&tmp, body, state.config.max_upload_bytes).await {
        let _ = tokio::fs::remove_file(&tmp).await;
        return Err(err.into());
    }
    let if_match = headers.get(header::IF_MATCH).and_then(|v| v.to_str().ok()).map(str::to_string);
    let store = state.store.clone();
    let tmp2 = tmp.clone();
    let id = actor.user_id;
    let result = blocking(move || store.write_file(id, &rel, &tmp2, if_match.as_deref())).await;
    let _ = tokio::fs::remove_file(&tmp).await;
    let entry = result?;
    let mut response = (StatusCode::OK, Json(write_result(&entry))).into_response();
    if let Ok(etag) = HeaderValue::from_str(&quote_etag(&entry.etag)) {
        response.headers_mut().insert(header::ETAG, etag);
    }
    Ok(response)
}

pub async fn upload(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Query(query): Query<PathQuery>,
    mut multipart: Multipart,
) -> Result<Json<FileEntry>, ApiError> {
    let dir = rel_of(query.path.as_deref())?;
    let mut created = None;
    while let Some(mut field) = multipart.next_field().await.map_err(|err| ApiError::new(StatusCode::BAD_REQUEST, err.to_string()))? {
        let raw_name = field.file_name().unwrap_or("upload").to_string();
        let base = std::path::Path::new(&raw_name).file_name().and_then(|s| s.to_str()).unwrap_or("upload");
        let name = single_name(base).map_err(ApiError::from)?;
        let tmp = state.store.tmp_root.join(format!("up-{}", random_id()));
        let mut file = tokio::fs::File::create(&tmp).await.map_err(crate::error::Error::from).map_err(ApiError::from)?;
        let mut total = 0u64;
        while let Some(chunk) = field.chunk().await.map_err(|err| ApiError::new(StatusCode::BAD_REQUEST, err.to_string()))? {
            total = total.saturating_add(chunk.len() as u64);
            if total > state.config.max_upload_bytes {
                let _ = tokio::fs::remove_file(&tmp).await;
                return Err(ApiError::from(crate::error::Error::TooLarge));
            }
            tokio::io::AsyncWriteExt::write_all(&mut file, &chunk).await.map_err(crate::error::Error::from).map_err(ApiError::from)?;
        }
        file.sync_all().await.map_err(crate::error::Error::from).map_err(ApiError::from)?;
        let rel = dir.join_name(&name).map_err(ApiError::from)?;
        let store = state.store.clone();
        let tmp2 = tmp.clone();
        let id = user.id;
        let result = blocking(move || store.write_file(id, &rel, &tmp2, None)).await;
        let _ = tokio::fs::remove_file(&tmp).await;
        created = Some(result?);
    }
    created.map(Json).ok_or_else(|| ApiError::new(StatusCode::BAD_REQUEST, "missing file"))
}

pub async fn mkdir(State(state): State<AppState>, AuthUser(user): AuthUser, Json(body): Json<PathBody>) -> Result<Json<FileEntry>, ApiError> {
    let rel = rel_of(Some(&body.path))?;
    let store = state.store.clone();
    let id = user.id;
    Ok(Json(blocking(move || store.mkdir(id, &rel)).await?))
}

pub async fn rename(State(state): State<AppState>, AuthUser(user): AuthUser, Json(body): Json<RenameBody>) -> Result<Json<FileEntry>, ApiError> {
    let rel = rel_of(Some(&body.path))?;
    let name = body.name;
    let store = state.store.clone();
    let id = user.id;
    Ok(Json(blocking(move || store.rename(id, &rel, &name)).await?))
}

pub async fn move_path(State(state): State<AppState>, AuthUser(user): AuthUser, Json(body): Json<TransferBody>) -> Result<Json<FileEntry>, ApiError> {
    let from = rel_of(Some(&body.from))?;
    let to_dir = rel_of(Some(&body.to_dir))?;
    let name = body.name;
    let store = state.store.clone();
    let id = user.id;
    Ok(Json(blocking(move || store.move_into(id, &from, &to_dir, name.as_deref())).await?))
}

pub async fn copy_path(State(state): State<AppState>, AuthUser(user): AuthUser, Json(body): Json<TransferBody>) -> Result<Json<FileEntry>, ApiError> {
    let from = rel_of(Some(&body.from))?;
    let to_dir = rel_of(Some(&body.to_dir))?;
    let name = body.name;
    let store = state.store.clone();
    let id = user.id;
    Ok(Json(blocking(move || store.copy(id, &from, &to_dir, name.as_deref())).await?))
}

pub async fn delete_path(State(state): State<AppState>, AuthUser(user): AuthUser, Json(body): Json<PathBody>) -> Result<StatusCode, ApiError> {
    let rel = rel_of(Some(&body.path))?;
    let store = state.store.clone();
    let id = user.id;
    blocking(move || store.trash(id, &rel)).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn list_trash(State(state): State<AppState>, AuthUser(user): AuthUser) -> Result<Json<Vec<TrashEntry>>, ApiError> {
    let rows = state.db.list_trash(user.id).map_err(ApiError::from)?;
    Ok(Json(rows.into_iter().map(trash_entry).collect()))
}

pub async fn restore_trash(State(state): State<AppState>, AuthUser(user): AuthUser, Path(id): Path<String>) -> Result<Json<FileEntry>, ApiError> {
    let store = state.store.clone();
    let owner = user.id;
    Ok(Json(blocking(move || store.restore(owner, &id)).await?))
}

pub async fn purge_trash(State(state): State<AppState>, AuthUser(user): AuthUser, Path(id): Path<String>) -> Result<StatusCode, ApiError> {
    let store = state.store.clone();
    let owner = user.id;
    blocking(move || store.purge(owner, &id)).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn empty_trash(State(state): State<AppState>, AuthUser(user): AuthUser) -> Result<StatusCode, ApiError> {
    let store = state.store.clone();
    let owner = user.id;
    blocking(move || store.empty_trash(owner)).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn file_response(path: std::path::PathBuf, entry: FileEntry) -> Result<Response, ApiError> {
    let file = tokio::fs::File::open(&path).await.map_err(crate::error::Error::from).map_err(ApiError::from)?;
    let body = axum::body::Body::from_stream(tokio_util::io::ReaderStream::new(file));
    let mut response = Response::new(body);
    let headers = response.headers_mut();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static(content_type(std::path::Path::new(&entry.name))));
    headers.insert(header::CONTENT_LENGTH, HeaderValue::from_str(&entry.size.to_string()).unwrap_or(HeaderValue::from_static("0")));
    if let Ok(value) = HeaderValue::from_str(&attachment_disposition(&entry.name)) {
        headers.insert(header::CONTENT_DISPOSITION, value);
    }
    if let Ok(value) = HeaderValue::from_str(&quote_etag(&entry.etag)) {
        headers.insert(header::ETAG, value);
    }
    headers.insert(header::X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
    Ok(response)
}

fn write_result(entry: &FileEntry) -> WriteResult {
    WriteResult { path: entry.path.clone(), etag: entry.etag.clone(), size: entry.size, is_dir: entry.is_dir }
}

fn trash_entry(row: crate::db::TrashRow) -> TrashEntry {
    TrashEntry {
        id: row.id,
        name: row.name,
        original_path: row.original_path,
        is_dir: row.is_dir,
        size: row.size,
        deleted_at: row.deleted_at,
    }
}

/// `GET /api/files/kind?path=`: which app a file belongs to by its content (LightCraft zips).
pub async fn file_kind(State(state): State<AppState>, AuthUser(user): AuthUser, Query(query): Query<PathQuery>) -> Result<Json<artcraft_cloud_types::FileKind>, ApiError> {
    let rel = rel_of(query.path.as_deref())?;
    let store = state.store.clone();
    let id = user.id;
    let kind = blocking(move || {
        let (path, _) = store.open_path(id, &rel)?;
        crate::sniff::kind_of(&path).map_err(crate::error::Error::from)
    })
    .await?;
    Ok(Json(kind))
}
