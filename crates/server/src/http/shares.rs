use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;

use super::blocking;
use super::files::{file_response, PathQuery};
use super::session::AuthUser;
use super::cookie_value;
use crate::db::ShareRow;
use crate::error::Error;
use crate::password::{hash_password, sign, verify_mac, verify_password};
use crate::paths::{now_secs, quote_etag, random_id, random_token, RelPath};
use crate::state::AppState;
use artcraft_cloud_types::{CreateShareRequest, DirListing, PublicShareView, Share, UnlockRequest, WriteResult};

pub async fn create_share(State(state): State<AppState>, AuthUser(user): AuthUser, Json(body): Json<CreateShareRequest>) -> Result<Json<Share>, ApiErrorCompat> {
    create_share_inner(state, user, body).await.map_err(Into::into)
}

async fn create_share_inner(state: AppState, user: crate::db::Account, body: CreateShareRequest) -> Result<Json<Share>, Error> {
    if body.permission != "view" && body.permission != "edit" {
        return Err(Error::Invalid("permission must be view or edit".into()));
    }
    let rel = RelPath::parse(&body.path)?;
    let stat = state.store.stat(user.id, &rel)?;
    if body.expires_at.is_some_and(|when| when <= now_secs()) {
        return Err(Error::Invalid("expiry is already in the past".into()));
    }
    let grantee_named = body.grantee.as_deref().map(str::trim).filter(|s| !s.is_empty());
    let group_named = body.group.as_deref().map(str::trim).filter(|s| !s.is_empty());
    if body.public && (grantee_named.is_some() || group_named.is_some()) {
        return Err(Error::Invalid("a public link does not take a grantee".into()));
    }
    if grantee_named.is_some() && group_named.is_some() {
        return Err(Error::Invalid("share with a person or a group, not both".into()));
    }
    if let Some(group_name) = group_named {
        let (group_id, _) = state.db.group_by_name(group_name)?.ok_or_else(|| Error::Invalid("no such group".into()))?;
        let id = match state.db.find_group_share(user.id, rel.as_str(), group_id)? {
            Some(existing) => {
                state.db.update_user_share(existing, &body.permission, body.expires_at)?;
                existing
            }
            None => state.db.insert_share(crate::db::NewShare {
                owner: user.id,
                path: rel.as_str(),
                grantee: None,
                group: Some(group_id),
                permission: &body.permission,
                token: None,
                password_hash: None,
                expires_at: body.expires_at,
            })?,
        };
        let row = state.db.share_by_id(id)?.ok_or(Error::NotFound)?;
        return Ok(Json(to_share(&row, stat.is_dir, true)));
    }
    if !body.public && grantee_named.is_none() {
        return Err(Error::Invalid("name the person to share with, or make a public link".into()));
    }
    let (grantee, token, password_hash) = if body.public {
        let password_hash = match body.password.as_deref().filter(|p| !p.is_empty()) {
            Some(password) => Some(hash_password(password, state.config.argon_memory_kib, state.config.argon_time, state.config.argon_parallelism)?),
            None => None,
        };
        (None, Some(random_token(24)), password_hash)
    } else {
        let name = crate::paths::normalize_username(grantee_named.unwrap_or(""))?;
        if name == user.username {
            return Err(Error::Invalid("share with someone else".into()));
        }
        let account = state.db.user_by_username(&name)?.ok_or_else(|| Error::Invalid("no such user".into()))?;
        if account.disabled {
            return Err(Error::Invalid("that user is disabled".into()));
        }
        (Some(account.id), None, None)
    };
    let id = if let Some(grantee_id) = grantee {
        if let Some(existing) = state.db.find_user_share(user.id, rel.as_str(), grantee_id)? {
            state.db.update_user_share(existing, &body.permission, body.expires_at)?;
            existing
        } else {
            state.db.insert_share(crate::db::NewShare {
                owner: user.id,
                path: rel.as_str(),
                grantee: Some(grantee_id),
                group: None,
                permission: &body.permission,
                token: None,
                password_hash: None,
                expires_at: body.expires_at,
            })?
        }
    } else {
        state.db.insert_share(crate::db::NewShare {
            owner: user.id,
            path: rel.as_str(),
            grantee: None,
            group: None,
            permission: &body.permission,
            token: token.as_deref(),
            password_hash: password_hash.as_deref(),
            expires_at: body.expires_at,
        })?
    };
    let row = state.db.share_by_id(id)?.ok_or(Error::NotFound)?;
    Ok(Json(to_share(&row, stat.is_dir, true)))
}

pub async fn list_shares(State(state): State<AppState>, AuthUser(user): AuthUser, Query(query): Query<BoxQuery>) -> Result<Json<Vec<Share>>, ApiErrorCompat> {
    let rows = if query.kind.as_deref() == Some("in") {
        state.db.shares_in(user.id).map_err(ApiErrorCompat::from)?
    } else {
        state.db.shares_out(user.id).map_err(ApiErrorCompat::from)?
    };
    let mut shares = Vec::new();
    for row in rows {
        let is_dir = state.store.stat(row.owner_id, &RelPath::parse(&row.path).map_err(ApiErrorCompat::from)?).map(|e| e.is_dir).unwrap_or(false);
        let include_token = row.owner_id == user.id;
        shares.push(to_share(&row, is_dir, include_token));
    }
    Ok(Json(shares))
}

#[derive(serde::Deserialize)]
pub struct BoxQuery {
    #[serde(rename = "box")]
    pub kind: Option<String>,
}

pub async fn delete_share(State(state): State<AppState>, AuthUser(user): AuthUser, Path(id): Path<i64>) -> Result<StatusCode, ApiErrorCompat> {
    state.db.delete_share(user.id, id).map_err(ApiErrorCompat::from)?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn list_shared(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(id): Path<i64>,
    Query(query): Query<PathQuery>,
) -> Result<Json<DirListing>, ApiErrorCompat> {
    let row = load_grantee(&state, id, user.id)?;
    let rel = RelPath::parse(query.path.as_deref().unwrap_or("")).map_err(ApiErrorCompat::from)?;
    let (owner, full, _) = resolve(&state, &row, &rel)?;
    listing(&state, owner, &RelPath::parse(&row.path).map_err(ApiErrorCompat::from)?, &full, &rel).map(Json).map_err(Into::into)
}

pub async fn read_shared(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(id): Path<i64>,
    Query(query): Query<PathQuery>,
) -> Result<Response, ApiErrorCompat> {
    let row = load_grantee(&state, id, user.id)?;
    let rel = RelPath::parse(query.path.as_deref().unwrap_or("")).map_err(ApiErrorCompat::from)?;
    let (owner, full, _) = resolve(&state, &row, &rel)?;
    let store = state.store.clone();
    let (path, entry) = blocking(move || store.open_path(owner, &full)).await?;
    file_response(path, entry).await.map_err(Into::into)
}

pub async fn write_shared(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(id): Path<i64>,
    Query(query): Query<PathQuery>,
    headers: HeaderMap,
    body: axum::body::Body,
) -> Result<Response, ApiErrorCompat> {
    let row = load_grantee(&state, id, user.id)?;
    if row.permission != "edit" {
        return Err(Error::Forbidden.into());
    }
    let rel = RelPath::parse(query.path.as_deref().unwrap_or("")).map_err(ApiErrorCompat::from)?;
    let (owner, full, _) = resolve(&state, &row, &rel)?;
    write_resolved(&state, owner, &full, headers, body).await.map_err(Into::into)
}

pub async fn public_view(
    State(state): State<AppState>,
    Path(token): Path<String>,
    Query(query): Query<PathQuery>,
    headers: HeaderMap,
) -> Result<Json<PublicShareView>, ApiErrorCompat> {
    let row = state.db.share_by_token(&token).map_err(ApiErrorCompat::from)?.ok_or(Error::NotFound).map_err(ApiErrorCompat::from)?;
    if row.expires_at.is_some_and(|when| when < now_secs()) {
        return Ok(Json(PublicShareView {
            token,
            permission: row.permission,
            needs_password: false,
            expired: true,
            is_dir: false,
            name: String::new(),
            listing: None,
        }));
    }
    let root = RelPath::parse(&row.path).map_err(ApiErrorCompat::from)?;
    let stat = state.store.stat(row.owner_id, &root).map_err(ApiErrorCompat::from)?;
    if row.password_hash.is_some() && !password_ok(&state, &token, &headers, row.password_hash.as_deref()) {
        return Ok(Json(PublicShareView {
            token,
            permission: row.permission,
            needs_password: true,
            expired: false,
            is_dir: stat.is_dir,
            name: stat.name,
            listing: None,
        }));
    }
    let rel = RelPath::parse(query.path.as_deref().unwrap_or("")).map_err(ApiErrorCompat::from)?;
    let (owner, full, _) = resolve(&state, &row, &rel)?;
    let listing = listing(&state, owner, &root, &full, &rel)?;
    let name = if stat.name.is_empty() { root.name().unwrap_or("shared").to_string() } else { stat.name };
    Ok(Json(PublicShareView {
        token,
        permission: row.permission,
        needs_password: false,
        expired: false,
        is_dir: stat.is_dir,
        name,
        listing: Some(listing),
    }))
}

pub async fn public_unlock(
    State(state): State<AppState>,
    Path(token): Path<String>,
    Json(body): Json<UnlockRequest>,
) -> Result<Response, ApiErrorCompat> {
    let row = state.db.share_by_token(&token).map_err(ApiErrorCompat::from)?.ok_or(Error::NotFound).map_err(ApiErrorCompat::from)?;
    if row.expires_at.is_some_and(|when| when < now_secs()) {
        return Err(ApiErrorCompat(crate::error::ApiError::new(StatusCode::GONE, "share expired")));
    }
    if !state.throttle.allow(&format!("share:{token}"), 8, 600) {
        return Err(ApiErrorCompat(crate::error::ApiError::new(StatusCode::TOO_MANY_REQUESTS, "too many attempts")));
    }
    let Some(hash) = row.password_hash.as_deref() else {
        return Ok(StatusCode::NO_CONTENT.into_response());
    };
    if !verify_password(&body.password, hash) {
        let _ = verify_password(&body.password, state.dummy_hash.as_str());
        return Err(ApiErrorCompat(crate::error::ApiError::new(StatusCode::UNAUTHORIZED, "wrong password")));
    }
    let exp = now_secs() + 3600;
    let sig = sign(&state.secret, &format!("{token}.{exp}")).map_err(ApiErrorCompat::from)?;
    let secure = if state.config.cookie_is_secure() { "; Secure" } else { "" };
    let cookie = format!("artcraft_unlock_{token}={exp}.{sig}; HttpOnly; SameSite=Lax; Path=/; Max-Age=3600{secure}");
    let mut response = StatusCode::NO_CONTENT.into_response();
    response.headers_mut().insert(
        header::SET_COOKIE,
        HeaderValue::from_str(&cookie).map_err(|_| ApiErrorCompat(crate::error::ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "cookie")))?,
    );
    Ok(response)
}

pub async fn public_read(
    State(state): State<AppState>,
    Path(token): Path<String>,
    Query(query): Query<PathQuery>,
    headers: HeaderMap,
) -> Result<Response, ApiErrorCompat> {
    let row = open_public(&state, &token, &headers)?;
    let rel = RelPath::parse(query.path.as_deref().unwrap_or("")).map_err(ApiErrorCompat::from)?;
    let (owner, full, _) = resolve(&state, &row, &rel)?;
    let store = state.store.clone();
    let (path, entry) = blocking(move || store.open_path(owner, &full)).await?;
    file_response(path, entry).await.map_err(Into::into)
}

pub async fn public_write(
    State(state): State<AppState>,
    Path(token): Path<String>,
    Query(query): Query<PathQuery>,
    headers: HeaderMap,
    body: axum::body::Body,
) -> Result<Response, ApiErrorCompat> {
    let row = open_public(&state, &token, &headers)?;
    if row.permission != "edit" {
        return Err(Error::Forbidden.into());
    }
    let rel = RelPath::parse(query.path.as_deref().unwrap_or("")).map_err(ApiErrorCompat::from)?;
    let (owner, full, _) = resolve(&state, &row, &rel)?;
    write_resolved(&state, owner, &full, headers, body).await.map_err(Into::into)
}

fn load_grantee(state: &AppState, id: i64, user: i64) -> Result<ShareRow, ApiErrorCompat> {
    let row = state.db.share_by_id(id).map_err(ApiErrorCompat::from)?.ok_or(Error::NotFound).map_err(ApiErrorCompat::from)?;
    let via_group = match row.group_id {
        Some(group) => state.db.in_group(user, group).map_err(ApiErrorCompat::from)?,
        None => false,
    };
    if row.grantee_id != Some(user) && row.owner_id != user && !via_group {
        return Err(Error::Forbidden.into());
    }
    if row.expires_at.is_some_and(|when| when < now_secs()) {
        return Err(ApiErrorCompat(crate::error::ApiError::new(StatusCode::GONE, "share expired")));
    }
    Ok(row)
}

fn open_public(state: &AppState, token: &str, headers: &HeaderMap) -> Result<ShareRow, ApiErrorCompat> {
    let row = state.db.share_by_token(token).map_err(ApiErrorCompat::from)?.ok_or(Error::NotFound).map_err(ApiErrorCompat::from)?;
    if row.expires_at.is_some_and(|when| when < now_secs()) {
        return Err(ApiErrorCompat(crate::error::ApiError::new(StatusCode::GONE, "share expired")));
    }
    if row.password_hash.is_some() && !password_ok(state, token, headers, row.password_hash.as_deref()) {
        return Err(ApiErrorCompat(crate::error::ApiError::unauthorized()));
    }
    Ok(row)
}

fn password_ok(state: &AppState, token: &str, headers: &HeaderMap, _hash: Option<&str>) -> bool {
    let header = headers.get(header::COOKIE).and_then(|v| v.to_str().ok()).unwrap_or("");
    let Some(value) = cookie_value(header, &format!("artcraft_unlock_{token}")) else {
        return false;
    };
    let Some((exp, sig)) = value.split_once('.') else {
        return false;
    };
    let Ok(exp_n) = exp.parse::<i64>() else {
        return false;
    };
    exp_n >= now_secs() && verify_mac(&state.secret, &format!("{token}.{exp}"), sig)
}

fn resolve(state: &AppState, row: &ShareRow, rel: &RelPath) -> Result<(i64, RelPath, String), Error> {
    let root = RelPath::parse(&row.path)?;
    let stat = state.store.stat(row.owner_id, &root)?;
    let full = if stat.is_dir {
        root.join_rel(rel)?
    } else if rel.is_root() {
        root
    } else {
        return Err(Error::NotFound);
    };
    Ok((row.owner_id, full, row.permission.clone()))
}

fn listing(state: &AppState, owner: i64, root: &RelPath, full: &RelPath, rel: &RelPath) -> Result<DirListing, Error> {
    let stat = state.store.stat(owner, full)?;
    let entries = if stat.is_dir {
        let mut entries = state.store.list(owner, full)?;
        for entry in &mut entries {
            let child = RelPath::parse(&entry.path)?;
            entry.path = relative_to(&child, root);
        }
        entries
    } else {
        let mut one = stat;
        one.path = relative_to(full, root);
        vec![one]
    };
    Ok(DirListing { path: rel.as_str().to_string(), entries })
}

fn relative_to(full: &RelPath, root: &RelPath) -> String {
    if full.as_str() == root.as_str() {
        return String::new();
    }
    full.as_str().strip_prefix(&format!("{}/", root.as_str())).unwrap_or(full.as_str()).to_string()
}

async fn write_resolved(state: &AppState, owner: i64, full: &RelPath, headers: HeaderMap, body: axum::body::Body) -> Result<Response, Error> {
    let tmp = state.store.tmp_root.join(format!("up-{}", random_id()));
    if let Err(err) = crate::webdav::write_body(&tmp, body, state.config.max_upload_bytes).await {
        let _ = tokio::fs::remove_file(&tmp).await;
        return Err(err);
    }
    let if_match = headers.get(header::IF_MATCH).and_then(|v| v.to_str().ok()).map(str::to_string);
    let store = state.store.clone();
    let tmp2 = tmp.clone();
    let full = full.clone();
    let result = blocking(move || store.write_file(owner, &full, &tmp2, if_match.as_deref())).await.map_err(|err| match err.status {
        StatusCode::CONFLICT => Error::Conflict { etag: err.etag.unwrap_or_default() },
        StatusCode::NOT_FOUND => Error::NotFound,
        StatusCode::FORBIDDEN => Error::Forbidden,
        StatusCode::BAD_REQUEST => Error::Invalid(err.message),
        StatusCode::PAYLOAD_TOO_LARGE => Error::TooLarge,
        _ => Error::Invalid(err.message),
    });
    let _ = tokio::fs::remove_file(&tmp).await;
    let entry = result?;
    let mut response = (StatusCode::OK, Json(WriteResult { path: entry.path.clone(), etag: entry.etag.clone(), size: entry.size, is_dir: entry.is_dir, rev: entry.rev })).into_response();
    if let Ok(etag) = HeaderValue::from_str(&quote_etag(&entry.etag)) {
        response.headers_mut().insert(header::ETAG, etag);
    }
    Ok(response)
}

fn to_share(row: &ShareRow, is_dir: bool, include_token: bool) -> Share {
    Share {
        id: row.id,
        path: row.path.clone(),
        owner: row.owner_name.clone(),
        grantee: row.grantee_name.clone(),
        permission: row.permission.clone(),
        public: row.token.is_some(),
        token: if include_token { row.token.clone() } else { None },
        has_password: row.password_hash.is_some(),
        expires_at: row.expires_at,
        is_dir,
        group: row.group_name.clone(),
    }
}

/// Local wrapper so handlers can return one error type that becomes a response.
pub struct ApiErrorCompat(pub crate::error::ApiError);

impl From<Error> for ApiErrorCompat {
    fn from(err: Error) -> Self {
        Self(err.into())
    }
}

impl From<crate::error::ApiError> for ApiErrorCompat {
    fn from(err: crate::error::ApiError) -> Self {
        Self(err)
    }
}

impl axum::response::IntoResponse for ApiErrorCompat {
    fn into_response(self) -> Response {
        self.0.into_response()
    }
}
