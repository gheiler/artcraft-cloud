//! The Apps page (super admin): the registry's apps, what is installed, and installs, updates,
//! rollbacks and removals without a new image or a restart. Also the catalog every page reads.

use std::path::PathBuf;

use axum::extract::{Path, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;

use artcraft_cloud_types::registry::{compare_versions, RegistryApp, Verdict};
use artcraft_cloud_types::{AdminApp, AdminApps, AdminVersion, AppAlert, AppInstallRequest, AppJob, AppSettingsRequest, AppStatus};

use super::session::AuthUser;
use super::users::require_super;
use crate::apps;
use crate::error::ApiError;
use crate::manifest::{read_manifest, version_dir, InstalledApp};
use crate::registry::{cloud_version, unix_now};
use crate::state::AppState;

/// Every app id artcraftCloud knows: the registry's, plus any installed that it no longer lists.
pub(crate) fn known_ids(state: &AppState) -> Vec<String> {
    let mut ids: Vec<String> = state.registry.current().map(|l| l.registry.apps.iter().map(|a| a.id.clone()).collect()).unwrap_or_default();
    for id in read_manifest(&state.config.data_dir).apps.into_keys() {
        if !ids.contains(&id) {
            ids.push(id);
        }
    }
    ids
}

/// The registry's entry for an app, when the registry in use lists it.
pub(crate) fn registry_app(state: &AppState, id: &str) -> Option<RegistryApp> {
    state.registry.current()?.registry.app(id).cloned()
}

fn status_of(app: &RegistryApp, row: Option<&InstalledApp>, full: bool) -> AppStatus {
    AppStatus {
        id: app.id.clone(),
        title: app.title.clone(),
        blurb: app.blurb.clone(),
        extensions: app.extensions.clone(),
        installed: row.is_some(),
        version: row.filter(|_| full).map(|r| r.version.clone()),
        previous: row.filter(|_| full).and_then(|r| r.previous.clone()),
        signature: row.filter(|_| full).map(|r| r.signature.clone()),
        publisher: app.publisher.clone(),
        bridge: app.bridge,
        author_query: app.author_query,
        chrome_query: app.chrome_query,
    }
}

/// The apps people can use: in the registry, and not disabled. `full` adds installed versions.
pub(crate) fn catalog(state: &AppState, full: bool) -> Vec<AppStatus> {
    let manifest = read_manifest(&state.config.data_dir);
    let Some(loaded) = state.registry.current() else { return Vec::new() };
    loaded
        .registry
        .apps
        .iter()
        .filter_map(|app| {
            let row = manifest.apps.get(&app.id).filter(|row| version_dir(&state.config.data_dir, &app.id, &row.version).join("index.html").is_file());
            if row.is_some_and(|r| r.disabled.is_some()) {
                return None;
            }
            // Apps that were never installed are offered on the Apps page, not in the launcher; the
            // default ones show while they download on a new install.
            if row.is_none() && !(app.default_install && !manifest.removed.contains(&app.id)) {
                return None;
            }
            Some(status_of(app, row, full))
        })
        .collect()
}

/// `GET /api/apps`: the catalog with what is installed (signed in).
pub async fn list(State(state): State<AppState>, AuthUser(_user): AuthUser) -> Json<Vec<AppStatus>> {
    Json(catalog(&state, true))
}

/// `GET /api/catalog`: the catalog without versions, for every page (shared links too).
pub async fn public_catalog(State(state): State<AppState>) -> Json<Vec<AppStatus>> {
    Json(catalog(&state, false))
}

/// `GET /api/apps/{id}/icon`: the registry's icon, else the one the shell ships.
pub async fn icon(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    if !artcraft_cloud_types::registry::valid_id(&id) {
        return StatusCode::NOT_FOUND.into_response();
    }
    let candidates: [PathBuf; 2] = [icon_path(&state, &id), state.config.shell_dir.join("assets").join("icons").join(format!("{id}.png"))];
    for path in candidates {
        if let Ok(bytes) = tokio::fs::read(&path).await {
            return ([(header::CONTENT_TYPE, "image/png"), (header::CACHE_CONTROL, "public, max-age=3600")], bytes).into_response();
        }
    }
    StatusCode::NOT_FOUND.into_response()
}

fn icon_path(state: &AppState, id: &str) -> PathBuf {
    state.config.data_dir.join("registry").join("icons").join(format!("{id}.png"))
}

/// Download the icons the registry names that are missing or changed (small, checked by hash).
pub(crate) async fn fetch_icons(state: &AppState) {
    let Some(loaded) = state.registry.current() else { return };
    let Ok(client) = reqwest::Client::builder().user_agent(crate::registry::USER_AGENT).timeout(std::time::Duration::from_secs(20)).build() else { return };
    for app in &loaded.registry.apps {
        let Some(icon) = &app.icon else { continue };
        let path = icon_path(state, &app.id);
        if apps::file_sha256(&path).is_ok_and(|sha| sha.eq_ignore_ascii_case(&icon.sha256)) {
            continue;
        }
        let Ok(response) = client.get(&icon.url).send().await else { continue };
        if !response.status().is_success() {
            continue;
        }
        let Ok(bytes) = response.bytes().await else { continue };
        if bytes.len() > 512 * 1024 || !hex::encode(<sha2::Sha256 as sha2::Digest>::digest(&bytes)).eq_ignore_ascii_case(&icon.sha256) {
            tracing::warn!(app = app.id, "icon does not match the registry; ignored");
            continue;
        }
        let _ = std::fs::create_dir_all(path.parent().unwrap_or(&path));
        let _ = crate::registry::write_atomic(&path, &bytes);
    }
}

/// Refresh the registry, apply its blocks and fetch icons (the periodic check and Refresh).
pub(crate) async fn refresh_and_enforce(state: &AppState) -> Result<(), String> {
    let result = state.registry.refresh().await.map(|_| ());
    if let Some(loaded) = state.registry.current() {
        apps::enforce(&state.config.data_dir, &loaded.registry, state.config.blocked_apps);
    }
    fetch_icons(state).await;
    result
}

/// Every six hours: the registry, its blocks, and the apps set to update by themselves. On start,
/// also the default apps a new install is missing.
pub async fn apps_loop(state: AppState) {
    let _ = refresh_and_enforce(&state).await;
    if !state.config.skip_app_sync {
        if let Some(loaded) = state.registry.current() {
            if let Err(err) = apps::sync_missing(&state.config.data_dir, &loaded).await {
                tracing::error!("{err}");
            }
        }
    }
    let mut tick = tokio::time::interval(std::time::Duration::from_secs(6 * 3600));
    tick.tick().await;
    loop {
        tick.tick().await;
        let _ = refresh_and_enforce(&state).await;
        if let Some(loaded) = state.registry.current() {
            for line in apps::auto_update(&state.config.data_dir, &loaded).await {
                tracing::info!("auto-update: {line}");
            }
        }
    }
}

pub(crate) fn admin_view(state: &AppState) -> AdminApps {
    let data = &state.config.data_dir;
    let manifest = read_manifest(data);
    let loaded = state.registry.current();
    let cloud = cloud_version();
    let jobs = state.app_jobs.lock().unwrap_or_else(|e| e.into_inner()).clone();
    let mut apps_out = Vec::new();
    let mut alerts = Vec::new();
    let empty = Vec::new();
    let registry_apps = loaded.as_ref().map(|l| &l.registry.apps).unwrap_or(&empty);
    let mut ids: Vec<String> = registry_apps.iter().map(|a| a.id.clone()).collect();
    ids.extend(manifest.apps.keys().filter(|id| !registry_apps.iter().any(|a| &a.id == *id)).cloned());
    for id in ids {
        let app = registry_apps.iter().find(|a| a.id == id);
        let row = manifest.apps.get(&id);
        let verdict = match (app, row) {
            (Some(app), Some(row)) => Some(app.verdict(&row.version, cloud)),
            (None, Some(_)) => Some(Verdict::Unlisted),
            _ => None,
        };
        let can_roll_back = row.is_some_and(|row| {
            row.previous.as_deref().is_some_and(|prev| version_dir(data, &id, prev).join("index.html").is_file() && app.is_none_or(|a| !matches!(a.verdict(prev, cloud), Verdict::Blocked { .. } | Verdict::Incompatible { .. })))
        });
        if let (Some(Verdict::Blocked { reason, security }), Some(row)) = (&verdict, row) {
            alerts.push(AppAlert {
                app: id.clone(),
                title: app.map(|a| a.title.clone()).unwrap_or_else(|| id.clone()),
                version: row.version.clone(),
                reason: reason.clone(),
                security: *security,
                action: row.disabled.as_ref().map(|_| "disabled".to_string()),
                to: None,
            });
        } else if let Some(row) = row.filter(|r| r.notice.is_some()) {
            // Rolled back automatically: the blocked version is now the previous one.
            let blocked = row.previous.clone().unwrap_or_default();
            let (reason, security) = match app.map(|a| a.verdict(&blocked, cloud)) {
                Some(Verdict::Blocked { reason, security }) => (reason, security),
                _ => (row.notice.clone().unwrap_or_default(), true),
            };
            alerts.push(AppAlert {
                app: id.clone(),
                title: app.map(|a| a.title.clone()).unwrap_or_else(|| id.clone()),
                version: blocked,
                reason,
                security,
                action: Some("rolled_back".into()),
                to: Some(row.version.clone()),
            });
        }
        let mut versions: Vec<AdminVersion> = app
            .map(|a| {
                a.versions
                    .iter()
                    .map(|v| {
                        let verdict = v.verdict(cloud);
                        AdminVersion {
                            version: v.version.clone(),
                            status: verdict.label().to_string(),
                            reason: verdict.reason(),
                            installable: verdict.allowed(),
                            recommended: a.recommended.as_deref() == Some(v.version.as_str()),
                            signed: v.signed.clone(),
                        }
                    })
                    .collect()
            })
            .unwrap_or_default();
        versions.sort_by(|a, b| compare_versions(&b.version, &a.version));
        apps_out.push(AdminApp {
            id: id.clone(),
            title: app.map(|a| a.title.clone()).unwrap_or_else(|| id.clone()),
            publisher: app.map(|a| a.publisher.clone()).unwrap_or_default(),
            blurb: app.map(|a| a.blurb.clone()).unwrap_or_default(),
            repo_url: app.map(|a| format!("https://github.com/{}/{}", a.owner, a.repo)).unwrap_or_default(),
            in_registry: app.is_some(),
            installed: row.map(|r| r.version.clone()),
            previous: row.and_then(|r| r.previous.clone()),
            can_roll_back,
            signature: row.map(|r| r.signature.clone()),
            state: verdict.as_ref().map(|v| v.label().to_string()).unwrap_or_else(|| "absent".into()),
            reason: verdict.as_ref().and_then(|v| v.reason()),
            disabled: row.and_then(|r| r.disabled.clone()),
            auto_update: row.is_some_and(|r| r.auto_update),
            notice: row.and_then(|r| r.notice.clone()),
            recommended: app.and_then(|a| a.target(cloud)).map(|v| v.version.clone()),
            latest: app.and_then(|a| a.latest_allowed(cloud)).map(|v| v.version.clone()),
            update: match (app, row) {
                (Some(a), Some(r)) => apps::update_for(a, &r.version),
                _ => None,
            },
            versions,
            job: jobs.get(&id).cloned(),
        });
    }
    AdminApps { registry: state.registry.status(), cloud_version: cloud.to_string(), apps: apps_out, alerts }
}

/// `GET /api/admin/apps`.
pub async fn admin_list(State(state): State<AppState>, AuthUser(user): AuthUser) -> Result<Json<AdminApps>, ApiError> {
    require_super(&user)?;
    Ok(Json(admin_view(&state)))
}

/// `POST /api/admin/apps/refresh`: fetch the registry now.
pub async fn admin_refresh(State(state): State<AppState>, AuthUser(user): AuthUser) -> Result<Json<AdminApps>, ApiError> {
    require_super(&user)?;
    // A failed fetch keeps the registry in use; the error shows in the view.
    let _ = refresh_and_enforce(&state).await;
    Ok(Json(admin_view(&state)))
}

fn set_job(state: &AppState, id: &str, job: AppJob) {
    state.app_jobs.lock().unwrap_or_else(|e| e.into_inner()).insert(id.to_string(), job);
}

/// `POST /api/admin/apps/{id}/install`: add an app or move it to another allowed version. Runs in
/// the background; the view shows its progress.
pub async fn admin_install(State(state): State<AppState>, AuthUser(user): AuthUser, Path(id): Path<String>, body: Option<Json<AppInstallRequest>>) -> Result<(StatusCode, Json<AdminApps>), ApiError> {
    require_super(&user)?;
    let version = body.and_then(|Json(b)| b.version).filter(|v| !v.trim().is_empty());
    let loaded = state.registry.current().ok_or_else(|| ApiError::new(StatusCode::SERVICE_UNAVAILABLE, "no app registry is available; refresh it"))?;
    let app = loaded.registry.app(&id).ok_or_else(|| ApiError::new(StatusCode::NOT_FOUND, "that app is not in the registry"))?;
    // Refuse what the registry does not allow before starting anything.
    let chosen = apps::choose(app, version.as_deref()).map_err(|err| ApiError::new(StatusCode::UNPROCESSABLE_ENTITY, err))?.version.clone();
    if loaded.registry.expired(unix_now()) {
        return Err(ApiError::new(StatusCode::CONFLICT, "the app registry has expired; refresh it before installing"));
    }
    {
        let jobs = state.app_jobs.lock().unwrap_or_else(|e| e.into_inner());
        if jobs.get(&id).is_some_and(|job| !job.done) {
            return Err(ApiError::new(StatusCode::CONFLICT, "this app is already being installed"));
        }
    }
    let action = match read_manifest(&state.config.data_dir).apps.get(&id) {
        Some(row) if compare_versions(&chosen, &row.version) == std::cmp::Ordering::Less => "downgrade",
        Some(_) => "update",
        None => "add",
    };
    let started = unix_now();
    set_job(&state, &id, AppJob { action: action.into(), version: Some(chosen.clone()), phase: "starting".into(), percent: None, error: None, done: false, started });
    let task_state = state.clone();
    let task_id = id.clone();
    tokio::spawn(async move {
        let progress_state = task_state.clone();
        let progress_id = task_id.clone();
        let progress: apps::Progress = std::sync::Arc::new(move |phase: &str, percent: Option<u8>| {
            let mut jobs = progress_state.app_jobs.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(job) = jobs.get_mut(&progress_id) {
                job.phase = phase.to_string();
                job.percent = percent;
            }
        });
        let result = apps::install(&task_state.config.data_dir, &loaded, &task_id, Some(&chosen), progress).await;
        let mut jobs = task_state.app_jobs.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(job) = jobs.get_mut(&task_id) {
            job.done = true;
            job.percent = None;
            match result {
                Ok(_) => job.phase = "done".into(),
                Err(err) => {
                    tracing::warn!(app = task_id, "install: {err}");
                    job.phase = "failed".into();
                    job.error = Some(err);
                }
            }
        }
    });
    Ok((StatusCode::ACCEPTED, Json(admin_view(&state))))
}

/// `POST /api/admin/apps/{id}/rollback`.
pub async fn admin_rollback(State(state): State<AppState>, AuthUser(user): AuthUser, Path(id): Path<String>) -> Result<Json<AdminApps>, ApiError> {
    require_super(&user)?;
    let loaded = state.registry.current();
    let data = state.config.data_dir.clone();
    let reg = loaded.as_ref().map(|l| l.registry.clone());
    let result = tokio::task::spawn_blocking(move || apps::rollback(&data, reg.as_ref(), &id)).await.map_err(|err| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, err.to_string()))?;
    result.map_err(|err| ApiError::new(StatusCode::CONFLICT, err))?;
    Ok(Json(admin_view(&state)))
}

/// `DELETE /api/admin/apps/{id}`: remove the app's files; people's documents stay.
pub async fn admin_remove(State(state): State<AppState>, AuthUser(user): AuthUser, Path(id): Path<String>) -> Result<Json<AdminApps>, ApiError> {
    require_super(&user)?;
    if state.app_jobs.lock().unwrap_or_else(|e| e.into_inner()).get(&id).is_some_and(|job| !job.done) {
        return Err(ApiError::new(StatusCode::CONFLICT, "this app is being installed"));
    }
    let data = state.config.data_dir.clone();
    tokio::task::spawn_blocking(move || apps::remove(&data, &id)).await.map_err(|err| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, err.to_string()))?.map_err(|err| ApiError::new(StatusCode::BAD_REQUEST, err))?;
    Ok(Json(admin_view(&state)))
}

/// `PUT /api/admin/apps/{id}/settings`: auto-update, and disable or enable.
pub async fn admin_settings(State(state): State<AppState>, AuthUser(user): AuthUser, Path(id): Path<String>, Json(body): Json<AppSettingsRequest>) -> Result<Json<AdminApps>, ApiError> {
    require_super(&user)?;
    if body.disabled == Some(false) {
        // Enabling a version the registry blocks is refused: roll back or update instead.
        if let (Some(app), Some(row)) = (registry_app(&state, &id), read_manifest(&state.config.data_dir).apps.get(&id)) {
            if let Verdict::Blocked { reason, .. } = app.verdict(&row.version, cloud_version()) {
                return Err(ApiError::new(StatusCode::CONFLICT, format!("{} is blocked by the registry ({reason}); update or roll back instead", row.version)));
            }
        }
    }
    apps::set_settings(&state.config.data_dir, &id, body.auto_update, body.disabled, false).map_err(|err| ApiError::new(StatusCode::BAD_REQUEST, err))?;
    Ok(Json(admin_view(&state)))
}

/// `POST /api/admin/apps/{id}/dismiss`: the admin read the notice.
pub async fn admin_dismiss(State(state): State<AppState>, AuthUser(user): AuthUser, Path(id): Path<String>) -> Result<Json<AdminApps>, ApiError> {
    require_super(&user)?;
    apps::set_settings(&state.config.data_dir, &id, None, None, true).map_err(|err| ApiError::new(StatusCode::BAD_REQUEST, err))?;
    state.app_jobs.lock().unwrap_or_else(|e| e.into_inner()).remove(&id);
    Ok(Json(admin_view(&state)))
}
