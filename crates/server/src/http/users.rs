use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;

use crate::error::ApiError;
use crate::manifest::read_manifest;
use crate::password::hash_password;
use crate::paths::{check_password_policy, display_name_of, normalize_username};
use crate::state::AppState;
use artcraft_cloud_types::{AdminPasswordReset, AppStatus, CreateUserRequest, User};

use super::session::AuthUser;

pub async fn list_users(State(state): State<AppState>, AuthUser(user): AuthUser) -> Result<Json<Vec<User>>, ApiError> {
    require_admin(&user)?;
    let users = state.db.list_users().map_err(ApiError::from)?;
    Ok(Json(users.into_iter().map(|account| account.public()).collect()))
}

pub async fn create_user(State(state): State<AppState>, AuthUser(actor): AuthUser, Json(body): Json<CreateUserRequest>) -> Result<(StatusCode, Json<User>), ApiError> {
    require_admin(&actor)?;
    let username = normalize_username(&body.username).map_err(ApiError::from)?;
    check_password_policy(&body.password).map_err(ApiError::from)?;
    let role = match body.role.as_deref() {
        Some("admin") => "admin",
        Some("user") | None => "user",
        Some(_) => return Err(ApiError::new(StatusCode::BAD_REQUEST, "role must be admin or user")),
    };
    let display = display_name_of(body.display_name.as_deref(), &username).map_err(ApiError::from)?;
    let hash = hash_password(&body.password, state.config.argon_memory_kib, state.config.argon_time, state.config.argon_parallelism).map_err(ApiError::from)?;
    let account = match state.db.insert_user(&username, &hash, role, &display) {
        Ok(account) => account,
        Err(crate::error::Error::Db(err)) if err.to_string().to_lowercase().contains("unique") => {
            return Err(ApiError::new(StatusCode::CONFLICT, "username is taken"));
        }
        Err(err) => return Err(err.into()),
    };
    state.store.ensure_home(account.id).map_err(ApiError::from)?;
    Ok((StatusCode::CREATED, Json(account.public())))
}

pub async fn disable_user(State(state): State<AppState>, AuthUser(actor): AuthUser, Path(id): Path<i64>) -> Result<StatusCode, ApiError> {
    set_enabled(&state, &actor, id, true).await
}

pub async fn enable_user(State(state): State<AppState>, AuthUser(actor): AuthUser, Path(id): Path<i64>) -> Result<StatusCode, ApiError> {
    set_enabled(&state, &actor, id, false).await
}

async fn set_enabled(state: &AppState, actor: &crate::db::Account, id: i64, disabled: bool) -> Result<StatusCode, ApiError> {
    require_admin(actor)?;
    if actor.id == id && disabled {
        return Err(ApiError::new(StatusCode::BAD_REQUEST, "you cannot disable your own account"));
    }
    let target = state.db.user_by_id(id).map_err(ApiError::from)?.ok_or_else(|| ApiError::new(StatusCode::NOT_FOUND, "not found"))?;
    if disabled && target.is_admin() && !target.disabled && state.db.enabled_admin_count().map_err(ApiError::from)? <= 1 {
        return Err(ApiError::new(StatusCode::BAD_REQUEST, "the last admin cannot be disabled"));
    }
    state.db.set_disabled(id, disabled).map_err(ApiError::from)?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn reset_password(
    State(state): State<AppState>,
    AuthUser(actor): AuthUser,
    Path(id): Path<i64>,
    Json(body): Json<AdminPasswordReset>,
) -> Result<StatusCode, ApiError> {
    require_admin(&actor)?;
    check_password_policy(&body.password).map_err(ApiError::from)?;
    let hash = hash_password(&body.password, state.config.argon_memory_kib, state.config.argon_time, state.config.argon_parallelism).map_err(ApiError::from)?;
    state.db.set_password(id, &hash).map_err(ApiError::from)?;
    state.basic_cache.clear();
    Ok(StatusCode::NO_CONTENT)
}

pub async fn apps(State(state): State<AppState>, AuthUser(_user): AuthUser) -> Json<Vec<AppStatus>> {
    let manifest = read_manifest(&state.config.data_dir);
    let apps = artcraft_cloud_types::catalog()
        .iter()
        .map(|app| {
            let installed = manifest.apps.get(app.id);
            AppStatus {
                id: app.id.to_string(),
                title: app.title.to_string(),
                blurb: app.blurb.to_string(),
                extensions: app.extensions.iter().map(|ext| (*ext).to_string()).collect(),
                installed: installed.is_some(),
                version: installed.map(|row| row.version.clone()),
                previous: installed.and_then(|row| row.previous.clone()),
                signature: installed.map(|row| row.signature.clone()),
            }
        })
        .collect();
    Json(apps)
}

fn require_admin(user: &crate::db::Account) -> Result<(), ApiError> {
    if user.is_admin() {
        Ok(())
    } else {
        Err(ApiError::new(StatusCode::FORBIDDEN, "admin only"))
    }
}
