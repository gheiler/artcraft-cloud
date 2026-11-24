use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;

use crate::error::ApiError;
use crate::manifest::read_manifest;
use crate::password::hash_password;
use crate::paths::{check_password_policy, display_name_of, normalize_username};
use crate::state::AppState;
use artcraft_cloud_types::{
    AdminPasswordReset, AppStatus, CreateGroupRequest, CreateUserRequest, Directory, DirectoryGroup, DirectoryUser, Group, GroupMemberRequest, PasswordChanged, SetRoleRequest, User,
};

use super::session::AuthUser;
use artcraft_cloud_types::prefs::{self, Preferences};

pub async fn list_users(State(state): State<AppState>, AuthUser(user): AuthUser) -> Result<Json<Vec<User>>, ApiError> {
    require_admin(&user)?;
    let users = state.db.list_users().map_err(ApiError::from)?;
    let mail = state.db.user_mail().map_err(ApiError::from)?;
    Ok(Json(
        users
            .into_iter()
            .map(|account| {
                let mut user = account.public();
                if let Some(info) = mail.get(&account.id) {
                    user.email = info.email.clone();
                    user.mailbox = info.mailbox.is_some();
                    user.mail_sync = info.sync.clone().unwrap_or_default();
                }
                user
            })
            .collect(),
    ))
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
    if role == "admin" {
        require_super(&actor)?;
    }
    let display = display_name_of(body.display_name.as_deref(), &username).map_err(ApiError::from)?;
    let email = match body.email.as_deref().map(str::trim).filter(|e| !e.is_empty()) {
        Some(email) => {
            let (email, _, _) = super::mail::normalize_email(email)?;
            if state.db.user_mail().map_err(ApiError::from)?.values().any(|info| info.email.as_deref() == Some(email.as_str())) {
                return Err(ApiError::new(StatusCode::CONFLICT, "another user has that email"));
            }
            Some(email)
        }
        None => None,
    };
    let hash = hash_password(&body.password, state.config.argon_memory_kib, state.config.argon_time, state.config.argon_parallelism).map_err(ApiError::from)?;
    let account = match state.db.insert_user(&username, &hash, role, &display) {
        Ok(account) => account,
        Err(crate::error::Error::Db(err)) if err.to_string().to_lowercase().contains("unique") => {
            return Err(ApiError::new(StatusCode::CONFLICT, "username is taken"));
        }
        Err(err) => return Err(err.into()),
    };
    state.store.ensure_home(account.id).map_err(ApiError::from)?;
    let mut user = account.public();
    if let Some(email) = email {
        // The account exists now; a mailbox that cannot be made is reported by the call the shell
        // makes next (`PUT /api/users/{id}/email`), so the email alone is set here.
        let request = artcraft_cloud_types::SetUserEmail { email: Some(email), create_mailbox: body.create_mailbox };
        // The admin just typed the password, so the mailbox gets it right away.
        match super::mail::set_email(&state, &account, &request, Some(&body.password)).await {
            Ok(mail) => {
                user.email = mail.email;
                user.mailbox = mail.mailbox.is_some();
                user.mail_sync = mail.password_sync;
            }
            Err(err) => {
                tracing::warn!(user = %account.username, error = %err.message, "mailbox not created");
                let _ = state.db.set_user_email(account.id, request.email.as_deref());
                user.email = request.email;
            }
        }
    }
    Ok((StatusCode::CREATED, Json(user)))
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
    if target.is_super && disabled {
        return Err(ApiError::new(StatusCode::BAD_REQUEST, "the super admin cannot be disabled"));
    }
    if target.is_admin() && actor.id != id {
        require_super(actor)?;
    }
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
) -> Result<Json<PasswordChanged>, ApiError> {
    require_admin(&actor)?;
    let target = state.db.user_by_id(id).map_err(ApiError::from)?.ok_or_else(|| ApiError::new(StatusCode::NOT_FOUND, "not found"))?;
    if target.is_admin() && actor.id != id {
        require_super(&actor)?;
    }
    check_password_policy(&body.password).map_err(ApiError::from)?;
    let hash = hash_password(&body.password, state.config.argon_memory_kib, state.config.argon_time, state.config.argon_parallelism).map_err(ApiError::from)?;
    state.db.set_password(id, &hash).map_err(ApiError::from)?;
    state.basic_cache.clear();
    Ok(Json(super::mail::sync_password(&state, id, &body.password).await))
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

/// `POST /api/users/{id}/role`: promote to admin or demote to user. Only the super admin may.
pub async fn set_role(
    State(state): State<AppState>,
    AuthUser(actor): AuthUser,
    Path(id): Path<i64>,
    Json(body): Json<SetRoleRequest>,
) -> Result<Json<User>, ApiError> {
    require_super(&actor)?;
    if body.role != "admin" && body.role != "user" {
        return Err(ApiError::new(StatusCode::BAD_REQUEST, "role must be admin or user"));
    }
    let target = state.db.user_by_id(id).map_err(ApiError::from)?.ok_or_else(|| ApiError::new(StatusCode::NOT_FOUND, "not found"))?;
    if target.is_super {
        return Err(ApiError::new(StatusCode::BAD_REQUEST, "the super admin stays an admin"));
    }
    state.db.set_role(id, &body.role).map_err(ApiError::from)?;
    let updated = state.db.user_by_id(id).map_err(ApiError::from)?.ok_or_else(|| ApiError::new(StatusCode::NOT_FOUND, "not found"))?;
    Ok(Json(updated.public()))
}

/// `GET /api/directory`: who the signed-in user can share with.
pub async fn directory(State(state): State<AppState>, AuthUser(user): AuthUser) -> Result<Json<Directory>, ApiError> {
    let users = state
        .db
        .list_users()
        .map_err(ApiError::from)?
        .into_iter()
        .filter(|account| !account.disabled && account.id != user.id)
        .map(|account| DirectoryUser { username: account.username, display_name: account.display_name })
        .collect();
    let groups = state
        .db
        .list_groups()
        .map_err(ApiError::from)?
        .into_iter()
        .map(|group| DirectoryGroup { name: group.name, members: group.members.len() })
        .collect();
    Ok(Json(Directory { users, groups }))
}

pub async fn list_groups(State(state): State<AppState>, AuthUser(user): AuthUser) -> Result<Json<Vec<Group>>, ApiError> {
    require_admin(&user)?;
    Ok(Json(state.db.list_groups().map_err(ApiError::from)?))
}

pub async fn create_group(State(state): State<AppState>, AuthUser(user): AuthUser, Json(body): Json<CreateGroupRequest>) -> Result<(StatusCode, Json<Group>), ApiError> {
    require_admin(&user)?;
    let name = body.name.trim();
    if name.is_empty() || name.chars().count() > 64 || name.chars().any(char::is_control) {
        return Err(ApiError::new(StatusCode::BAD_REQUEST, "a group name is 1 to 64 characters"));
    }
    let id = match state.db.create_group(name) {
        Ok(id) => id,
        Err(crate::error::Error::Db(err)) if err.to_string().to_lowercase().contains("unique") => {
            return Err(ApiError::new(StatusCode::CONFLICT, "a group with that name exists"));
        }
        Err(err) => return Err(err.into()),
    };
    Ok((StatusCode::CREATED, Json(Group { id, name: name.to_string(), members: Vec::new() })))
}

pub async fn delete_group(State(state): State<AppState>, AuthUser(user): AuthUser, Path(id): Path<i64>) -> Result<StatusCode, ApiError> {
    require_admin(&user)?;
    state.db.delete_group(id).map_err(ApiError::from)?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn add_member(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(id): Path<i64>,
    Json(body): Json<GroupMemberRequest>,
) -> Result<StatusCode, ApiError> {
    require_admin(&user)?;
    let member = member_of(&state, id, &body.username)?;
    state.db.add_member(id, member).map_err(ApiError::from)?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn remove_member(State(state): State<AppState>, AuthUser(user): AuthUser, Path((id, username)): Path<(i64, String)>) -> Result<StatusCode, ApiError> {
    require_admin(&user)?;
    let member = member_of(&state, id, &username)?;
    state.db.remove_member(id, member).map_err(ApiError::from)?;
    Ok(StatusCode::NO_CONTENT)
}

fn member_of(state: &AppState, group: i64, username: &str) -> Result<i64, ApiError> {
    if !state.db.list_groups().map_err(ApiError::from)?.iter().any(|g| g.id == group) {
        return Err(ApiError::new(StatusCode::NOT_FOUND, "no such group"));
    }
    let account = state.db.user_by_username(username.trim()).map_err(ApiError::from)?.ok_or_else(|| ApiError::new(StatusCode::BAD_REQUEST, "no such user"))?;
    Ok(account.id)
}

/// Largest settings blob one app may store for one user.
pub const SETTINGS_LIMIT: usize = 1024 * 1024;

fn settings_key(app: &str) -> Result<&str, ApiError> {
    if app == "shell" || artcraft_cloud_types::catalog().iter().any(|row| row.id == app) {
        Ok(app)
    } else {
        Err(ApiError::new(StatusCode::NOT_FOUND, "no such app"))
    }
}

/// `GET /api/settings/{app}`: the JSON object the shell stored for this user and app, or `{}`.
pub async fn get_settings(State(state): State<AppState>, AuthUser(user): AuthUser, Path(app): Path<String>) -> Result<axum::response::Response, ApiError> {
    use axum::response::IntoResponse;
    let key = settings_key(&app)?;
    let data = state.db.setting(user.id, key).map_err(ApiError::from)?.unwrap_or_else(|| "{}".to_string());
    Ok(([(axum::http::header::CONTENT_TYPE, "application/json"), (axum::http::header::CACHE_CONTROL, "no-store")], data).into_response())
}

/// `PUT /api/settings/{app}`: replace the stored settings. The body must be a JSON object.
pub async fn put_settings(State(state): State<AppState>, AuthUser(user): AuthUser, Path(app): Path<String>, body: axum::body::Bytes) -> Result<StatusCode, ApiError> {
    let key = settings_key(&app)?;
    if body.len() > SETTINGS_LIMIT {
        return Err(ApiError::new(StatusCode::PAYLOAD_TOO_LARGE, "settings are limited to 1 MiB per app"));
    }
    let mut value: serde_json::Value = serde_json::from_slice(&body).map_err(|_| ApiError::new(StatusCode::BAD_REQUEST, "settings must be JSON"))?;
    if !value.is_object() {
        return Err(ApiError::new(StatusCode::BAD_REQUEST, "settings must be a JSON object"));
    }
    if key == "mailcraft" {
        // MailCraft keeps the mail itself in `localStorage` too; only its options are stored here.
        if let Some(map) = value.as_object_mut() {
            map.remove("mailcraft.mailbox.v1");
        }
    }
    state.db.put_setting(user.id, key, &value.to_string()).map_err(ApiError::from)?;
    Ok(StatusCode::NO_CONTENT)
}

pub(crate) fn require_super(user: &crate::db::Account) -> Result<(), ApiError> {
    if user.is_admin() && user.is_super {
        Ok(())
    } else {
        Err(ApiError::new(StatusCode::FORBIDDEN, "only the super admin can manage admins"))
    }
}

pub(crate) fn require_admin(user: &crate::db::Account) -> Result<(), ApiError> {
    if user.is_admin() {
        Ok(())
    } else {
        Err(ApiError::new(StatusCode::FORBIDDEN, "admin only"))
    }
}

/// Where the common settings are kept in `app_settings` (not reachable through `/api/settings`).
const PREFERENCES_KEY: &str = "preferences";

fn load_preferences(state: &AppState, user: i64) -> Result<Preferences, ApiError> {
    let stored = state.db.setting(user, PREFERENCES_KEY).map_err(ApiError::from)?;
    Ok(stored.and_then(|text| serde_json::from_str::<Preferences>(&text).ok()).unwrap_or_default().normalized())
}

/// `GET /api/preferences`: the user's common settings (defaults until they change one).
pub async fn get_preferences(State(state): State<AppState>, AuthUser(user): AuthUser) -> Result<Json<Preferences>, ApiError> {
    Ok(Json(load_preferences(&state, user.id)?))
}

/// `PUT /api/preferences`: store the common settings and write every changed one into each app's
/// stored options ([`artcraft_cloud_types::prefs::apply`]), so the apps take it on their next start.
/// Returns the stored settings.
pub async fn put_preferences(State(state): State<AppState>, AuthUser(user): AuthUser, Json(body): Json<serde_json::Value>) -> Result<Json<Preferences>, ApiError> {
    if !body.is_object() {
        return Err(ApiError::new(StatusCode::BAD_REQUEST, "preferences must be a JSON object"));
    }
    let next = serde_json::from_value::<Preferences>(body).map_err(|_| ApiError::new(StatusCode::BAD_REQUEST, "preferences do not fit"))?.normalized();
    let before = load_preferences(&state, user.id)?;
    let changed = before.changed(&next);
    let text = serde_json::to_string(&next).map_err(|err| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, err.to_string()))?;
    state.db.put_setting(user.id, PREFERENCES_KEY, &text).map_err(ApiError::from)?;
    if !changed.is_empty() {
        for app in artcraft_cloud_types::catalog() {
            let stored = state.db.setting(user.id, app.id).map_err(ApiError::from)?;
            let mut map = match stored.as_deref().map(serde_json::from_str::<serde_json::Value>) {
                Some(Ok(serde_json::Value::Object(map))) => map,
                Some(_) => continue,
                None => serde_json::Map::new(),
            };
            if prefs::apply(app.id, &next, &changed, &user.display_name, prefs::Mode::Override, &mut map) {
                let text = serde_json::Value::Object(map).to_string();
                if text.len() <= SETTINGS_LIMIT {
                    state.db.put_setting(user.id, app.id, &text).map_err(ApiError::from)?;
                }
            }
        }
    }
    Ok(Json(next))
}
