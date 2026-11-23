use axum::extract::{FromRequestParts, State};
use axum::http::request::Parts;
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;

use super::{blocking, clear_cookie, cookie_value, session_cookie};
use crate::db::Account;
use crate::error::ApiError;
use crate::password::{hash_password, verify_password};
use crate::paths::{check_password_policy, normalize_username, random_token};
use crate::state::AppState;
use artcraft_cloud_types::{LoginRequest, PasswordChange, User};

pub struct AuthUser(pub Account);

impl FromRequestParts<AppState> for AuthUser {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, Self::Rejection> {
        let header = parts.headers.get(header::COOKIE).and_then(|value| value.to_str().ok()).unwrap_or("");
        let token = cookie_value(header, "artcraft_session").ok_or_else(ApiError::unauthorized)?;
        let user = state.db.session_user(&token).map_err(ApiError::from)?.ok_or_else(ApiError::unauthorized)?;
        if user.disabled {
            return Err(ApiError::unauthorized());
        }
        Ok(AuthUser(user))
    }
}

pub async fn login(State(state): State<AppState>, Json(body): Json<LoginRequest>) -> Result<Response, ApiError> {
    let username = normalize_username(&body.username).map_err(|_| invalid_login())?;
    if body.password.len() > 1024 || !state.throttle.allow(&format!("login:{username}"), 8, 600) {
        return Err(ApiError::new(StatusCode::TOO_MANY_REQUESTS, "too many attempts"));
    }
    let stored = state.db.user_by_username(&username).map_err(ApiError::from)?;
    let hash = stored.as_ref().map(|account| account.password_hash.clone()).unwrap_or_else(|| state.dummy_hash.to_string());
    let matches = verify_password(&body.password, &hash);
    let Some(account) = stored else {
        return Err(invalid_login());
    };
    if account.disabled || !matches {
        return Err(invalid_login());
    }
    let token = random_token(32);
    state.db.insert_session(&token, account.id, state.config.session_ttl_secs).map_err(ApiError::from)?;
    state.store.ensure_home(account.id).map_err(ApiError::from)?;
    let mut response = Json(account.public()).into_response();
    let cookie = session_cookie(&token, state.config.session_ttl_secs, state.config.cookie_is_secure());
    response.headers_mut().insert(header::SET_COOKIE, HeaderValue::from_str(&cookie).map_err(|_| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "cookie"))?);
    Ok(response)
}

pub async fn logout(State(state): State<AppState>, headers: axum::http::HeaderMap) -> Result<Response, ApiError> {
    if let Some(token) = headers.get(header::COOKIE).and_then(|v| v.to_str().ok()).and_then(|h| cookie_value(h, "artcraft_session")) {
        state.db.delete_session(&token).map_err(ApiError::from)?;
    }
    let mut response = StatusCode::NO_CONTENT.into_response();
    let cookie = clear_cookie(state.config.cookie_is_secure());
    response.headers_mut().insert(header::SET_COOKIE, HeaderValue::from_str(&cookie).map_err(|_| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "cookie"))?);
    Ok(response)
}

pub async fn me(AuthUser(user): AuthUser) -> Json<User> {
    Json(user.public())
}

pub async fn change_password(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Json(body): Json<PasswordChange>,
) -> Result<Response, ApiError> {
    check_password_policy(&body.next).map_err(ApiError::from)?;
    if !verify_password(&body.current, &user.password_hash) {
        return Err(ApiError::new(StatusCode::UNAUTHORIZED, "invalid username or password"));
    }
    let hash = hash_password(&body.next, state.config.argon_memory_kib, state.config.argon_time, state.config.argon_parallelism).map_err(ApiError::from)?;
    let db = state.db.clone();
    let id = user.id;
    blocking(move || db.set_password(id, &hash)).await?;
    state.basic_cache.clear();
    let token = random_token(32);
    state.db.insert_session(&token, user.id, state.config.session_ttl_secs).map_err(ApiError::from)?;
    let mut response = StatusCode::NO_CONTENT.into_response();
    let cookie = session_cookie(&token, state.config.session_ttl_secs, state.config.cookie_is_secure());
    response.headers_mut().insert(header::SET_COOKIE, HeaderValue::from_str(&cookie).map_err(|_| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "cookie"))?);
    Ok(response)
}

fn invalid_login() -> ApiError {
    ApiError::new(StatusCode::UNAUTHORIZED, "invalid username or password")
}
