//! The Apps page (super admin): the registry's apps, what is installed, and installs, updates,
//! rollbacks and removals without a new image or a restart. Also the catalog every page reads.

use std::path::PathBuf;

use axum::extract::{Path, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;

use artcraft_cloud_types::registry::{compare_versions, RegistryApp, Verdict};
use artcraft_cloud_types::{AdminApp, AdminApps, AdminVersion, AppAlert, AppInstallRequest, AppJob, AppSettingsRequest, AppSource, AppStatus, SideloadRequest, TypeConflict};

use super::blocking;
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

fn official_grants(app: &RegistryApp) -> Vec<String> {
    let mut grants = vec!["clipboard".into(), "fullscreen".into()];
    if app.bridge {
        grants.insert(0, "document".into());
        grants.insert(1, "drive".into());
    }
    match app.id.as_str() {
        "mailcraft" => grants.insert(0, "mail".into()),
        "calendarcraft" => grants.insert(0, "calendar".into()),
        _ => {}
    }
    grants
}

fn status_of(_state: &AppState, manifest: &crate::manifest::Manifest, app: &RegistryApp, row: Option<&InstalledApp>, full: bool, acks: Option<&[String]>) -> AppStatus {
    let unverified = row.is_some_and(|r| r.is_sideload());
    let extensions = if unverified {
        // Official types win once the registry lists this id.
        app.extensions.clone()
    } else {
        app.extensions.clone()
    };
    let defaults = extensions.iter().filter(|ext| manifest.defaults.get(*ext).map(String::as_str) == Some(app.id.as_str()) || (!manifest.defaults.contains_key(*ext) && row.is_none_or(|r| !r.is_sideload()))).cloned().collect();
    let grants = row.and_then(|r| r.sideload.as_ref()).map(|m| m.grants.clone()).unwrap_or_else(|| official_grants(app));
    let isolation = if unverified {
        row.and_then(|r| r.sideload.as_ref()).map(crate::sideload::isolation_of).unwrap_or_else(|| artcraft_cloud_types::ISOLATION_SANDBOX.to_string())
    } else {
        String::new()
    };
    AppStatus {
        id: app.id.clone(),
        title: app.title.clone(),
        blurb: app.blurb.clone(),
        extensions,
        installed: row.is_some(),
        version: row.filter(|_| full).map(|r| r.version.clone()),
        previous: row.filter(|_| full).and_then(|r| r.previous.clone()),
        signature: row.filter(|_| full).map(|r| r.signature.clone()),
        publisher: app.publisher.clone(),
        bridge: app.bridge,
        author_query: app.author_query,
        chrome_query: app.chrome_query,
        unverified,
        isolated: unverified && artcraft_cloud_types::isolation_is_isolated(&isolation),
        isolation,
        defaults,
        grants,
        needs_ack: full && unverified && acks.is_none_or(|a| !a.iter().any(|id| id == &app.id)),
    }
}

fn sideload_status(state: &AppState, manifest: &crate::manifest::Manifest, id: &str, row: &InstalledApp, full: bool, acks: Option<&[String]>) -> Option<AppStatus> {
    let meta = row.sideload.as_ref()?;
    if row.disabled.is_some() {
        return None;
    }
    if !version_dir(&state.config.data_dir, id, &row.version).join("index.html").is_file() {
        return None;
    }
    let defaults = meta.extensions.iter().filter(|ext| manifest.defaults.get(*ext).map(String::as_str) == Some(id)).cloned().collect();
    let isolation = crate::sideload::isolation_of(meta);
    Some(AppStatus {
        id: id.to_string(),
        title: meta.name.clone(),
        blurb: meta.blurb.clone(),
        extensions: meta.extensions.clone(),
        installed: true,
        version: full.then(|| row.version.clone()),
        previous: row.previous.clone().filter(|_| full),
        signature: full.then(|| row.signature.clone()),
        publisher: meta.publisher.clone(),
        bridge: meta.bridge,
        author_query: false,
        chrome_query: false,
        unverified: true,
        isolated: artcraft_cloud_types::isolation_is_isolated(&isolation),
        isolation,
        defaults,
        grants: meta.grants.clone(),
        needs_ack: full && acks.is_none_or(|a| !a.iter().any(|ack| ack == id)),
    })
}

/// The apps people can use: in the registry, and not disabled. `full` adds installed versions.
pub(crate) fn catalog(state: &AppState, full: bool, acks: Option<&[String]>) -> Vec<AppStatus> {
    let manifest = read_manifest(&state.config.data_dir);
    let mut out = Vec::new();
    if let Some(loaded) = state.registry.current() {
        for app in &loaded.registry.apps {
            let row = manifest.apps.get(&app.id).filter(|row| version_dir(&state.config.data_dir, &app.id, &row.version).join("index.html").is_file());
            if row.is_some_and(|r| r.disabled.is_some()) {
                continue;
            }
            // Official metadata wins if the registry lists this id, even while a sideload is still installed.
            if row.is_some_and(|r| r.is_sideload()) {
                out.push(status_of(state, &manifest, app, row, full, acks));
                if let Some(status) = out.last_mut() {
                    status.unverified = true;
                    let isolation = row.and_then(|r| r.sideload.as_ref()).map(crate::sideload::isolation_of).unwrap_or_else(|| artcraft_cloud_types::ISOLATION_SANDBOX.to_string());
                    status.isolated = artcraft_cloud_types::isolation_is_isolated(&isolation);
                    status.isolation = isolation;
                    status.needs_ack = full && acks.is_none_or(|a| !a.iter().any(|id| id == &app.id));
                }
                continue;
            }
            // Apps that were never installed are offered on the Apps page, not in the launcher; the
            // default ones show while they download on a new install.
            if row.is_none() && !(app.default_install && !manifest.removed.contains(&app.id)) {
                continue;
            }
            out.push(status_of(state, &manifest, app, row, full, acks));
        }
    }
    for (id, row) in &manifest.apps {
        if out.iter().any(|app| app.id == *id) {
            continue;
        }
        if let Some(status) = sideload_status(state, &manifest, id, row, full, acks) {
            out.push(status);
        }
    }
    out
}

/// `GET /api/apps`: the catalog with what is installed (signed in).
pub async fn list(State(state): State<AppState>, AuthUser(user): AuthUser) -> Json<Vec<AppStatus>> {
    let acks = state.db.unverified_acks(user.id).unwrap_or_default();
    Json(catalog(&state, true, Some(&acks)))
}

/// `GET /api/catalog`: the catalog without versions, for every page (shared links too).
pub async fn public_catalog(State(state): State<AppState>) -> Json<Vec<AppStatus>> {
    Json(catalog(&state, false, None))
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
        apps::refresh_defaults(&state.config.data_dir, Some(&loaded.registry));
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
        check_sideload_sources(&state).await;
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
            unverified: row.is_some_and(|r| r.is_sideload()),
            extensions: row.and_then(|r| r.sideload.as_ref()).map(|m| m.extensions.clone()).or_else(|| app.map(|a| a.extensions.clone())).unwrap_or_default(),
            grants: row.and_then(|r| r.sideload.as_ref()).map(|m| m.grants.clone()).unwrap_or_default(),
            source: row.and_then(|r| r.sideload.as_ref()).and_then(|m| m.source.as_ref()).map(|s| AppSource { kind: s.kind.clone(), url: s.url.clone() }),
            sha256: row.map(|r| r.sha256.clone()),
            official_available: row.is_some_and(|r| r.is_sideload()) && app.is_some(),
            claim_defaults: row.and_then(|r| r.sideload.as_ref()).is_some_and(|m| m.claim_defaults),
            defaults: row.and_then(|r| r.sideload.as_ref()).map(|m| m.extensions.iter().filter(|ext| manifest.defaults.get(*ext).map(String::as_str) == Some(id.as_str())).cloned().collect()).unwrap_or_default(),
            isolation: row.and_then(|r| r.sideload.as_ref()).map(crate::sideload::isolation_of).unwrap_or_default(),
        });
        // A source check can offer a version the registry does not list.
        if let Some(offered) = row.and_then(|r| r.sideload.as_ref()).and_then(|m| m.offered.as_ref()) {
            if let Some(last) = apps_out.last_mut() {
                if last.update.is_none() && artcraft_cloud_types::registry::compare_versions(&offered.version, row.map(|r| r.version.as_str()).unwrap_or("")) == std::cmp::Ordering::Greater {
                    last.update = Some(offered.version.clone());
                }
                if !last.versions.iter().any(|v| v.version == offered.version) {
                    last.versions.insert(0, AdminVersion {
                        version: offered.version.clone(),
                        status: "ok".into(),
                        reason: None,
                        installable: true,
                        recommended: last.update.as_deref() == Some(offered.version.as_str()),
                        signed: match offered.signed.as_str() { "" | "none" => "source".into(), other => other.to_string() },
                    });
                }
            }
        }
        if let Some(last) = apps_out.last_mut() {
            if last.title == id {
                if let Some(name) = row.and_then(|r| r.sideload.as_ref()).map(|m| m.name.clone()) {
                    last.title = name;
                    last.publisher = row.and_then(|r| r.sideload.as_ref()).map(|m| m.publisher.clone()).unwrap_or_default();
                    last.blurb = row.and_then(|r| r.sideload.as_ref()).map(|m| m.blurb.clone()).unwrap_or_default();
                }
            }
        }
    }
    AdminApps {
        registry: state.registry.status(),
        cloud_version: cloud.to_string(),
        apps: apps_out,
        alerts,
        audit: state.db.list_app_audit(80).unwrap_or_default(),
        apps_origin: state.config.apps_origin().map(str::to_string),
    }
}

#[allow(clippy::too_many_arguments)]
fn audit(state: &AppState, actor: &str, action: &str, app: &str, version: Option<&str>, sha256: Option<&str>, source: Option<&str>, detail: Option<&str>) {
    if let Err(err) = state.db.append_app_audit(actor, action, app, version, sha256, source, detail) {
        tracing::warn!("app audit log: {err}");
    }
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
    if registry_app(&state, &id).is_none() {
        return start_source_install(state, &user.username, id, version).await;
    }
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
    let actor = user.username.clone();
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
                Ok((version, signature)) => {
                    job.phase = "done".into();
                    let sha = read_manifest(&task_state.config.data_dir).apps.get(&task_id).map(|r| r.sha256.clone());
                    drop(jobs);
                    audit(&task_state, &actor, if action == "add" { "install" } else { "update" }, &task_id, Some(&version), sha.as_deref(), Some("registry"), Some(&signature));
                }
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
    let app_id = id.clone();
    let result = tokio::task::spawn_blocking(move || apps::rollback(&data, reg.as_ref(), &id)).await.map_err(|err| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, err.to_string()))?;
    let version = result.map_err(|err| ApiError::new(StatusCode::CONFLICT, err))?;
    let sha = read_manifest(&state.config.data_dir).apps.get(&app_id).map(|r| r.sha256.clone());
    audit(&state, &user.username, "rollback", &app_id, Some(&version), sha.as_deref(), None, None);
    Ok(Json(admin_view(&state)))
}

/// `DELETE /api/admin/apps/{id}`: remove the app's files; people's documents stay.
pub async fn admin_remove(State(state): State<AppState>, AuthUser(user): AuthUser, Path(id): Path<String>) -> Result<Json<AdminApps>, ApiError> {
    require_super(&user)?;
    if state.app_jobs.lock().unwrap_or_else(|e| e.into_inner()).get(&id).is_some_and(|job| !job.done) {
        return Err(ApiError::new(StatusCode::CONFLICT, "this app is being installed"));
    }
    let data = state.config.data_dir.clone();
    let app_id = id.clone();
    let row = read_manifest(&data).apps.get(&id).cloned();
    tokio::task::spawn_blocking(move || apps::remove(&data, &id)).await.map_err(|err| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, err.to_string()))?.map_err(|err| ApiError::new(StatusCode::BAD_REQUEST, err))?;
    audit(&state, &user.username, "remove", &app_id, row.as_ref().map(|r| r.version.as_str()), row.as_ref().map(|r| r.sha256.as_str()), row.as_ref().and_then(|r| r.sideload.as_ref()).and_then(|m| m.source.as_ref()).map(|s| s.url.as_str()), None);
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
    apply_settings(&state, &user, &id, body)?;
    Ok(Json(admin_view(&state)))
}

/// `POST /api/admin/apps/{id}/dismiss`: the admin read the notice.
pub async fn admin_dismiss(State(state): State<AppState>, AuthUser(user): AuthUser, Path(id): Path<String>) -> Result<Json<AdminApps>, ApiError> {
    require_super(&user)?;
    apps::set_settings(&state.config.data_dir, &id, None, None, true).map_err(|err| ApiError::new(StatusCode::BAD_REQUEST, err))?;
    state.app_jobs.lock().unwrap_or_else(|e| e.into_inner()).remove(&id);
    Ok(Json(admin_view(&state)))
}

fn apply_settings(state: &AppState, user: &crate::db::Account, id: &str, body: AppSettingsRequest) -> Result<(), ApiError> {
    let before = read_manifest(&state.config.data_dir).apps.get(id).cloned();
    apps::set_settings(&state.config.data_dir, id, body.auto_update, body.disabled, false).map_err(|err| ApiError::new(StatusCode::BAD_REQUEST, err))?;
    let mut isolation_detail = None;
    let mut isolation_only = false;
    if body.extensions.is_some() || body.grants.is_some() || body.claim_defaults.is_some() || body.isolation.is_some() {
        let manifest = read_manifest(&state.config.data_dir);
        let row = manifest.apps.get(id).ok_or_else(|| ApiError::new(StatusCode::NOT_FOUND, "that app is not installed"))?;
        if !row.is_sideload() {
            return Err(ApiError::new(StatusCode::BAD_REQUEST, "file types, grants and isolation can only be changed on a sideloaded app"));
        }
        let meta = row.sideload.as_ref().ok_or_else(|| ApiError::new(StatusCode::BAD_REQUEST, "not a sideloaded app"))?;
        let current_isolation = crate::sideload::isolation_of(meta);
        let extensions = match &body.extensions {
            Some(list) => crate::sideload::normalize_extensions(list).map_err(|err| ApiError::new(StatusCode::BAD_REQUEST, err))?,
            None => meta.extensions.clone(),
        };
        let grants = match &body.grants {
            Some(list) => crate::sideload::normalize_grants(list, meta.bridge).map_err(|err| ApiError::new(StatusCode::BAD_REQUEST, err))?,
            None => meta.grants.clone(),
        };
        let claim = body.claim_defaults.unwrap_or(meta.claim_defaults);
        let isolation = match &body.isolation {
            Some(value) => {
                let confirm = body.confirm_regular == Some(true) || current_isolation == artcraft_cloud_types::ISOLATION_REGULAR;
                let mode = crate::sideload::normalize_isolation(Some(value), state.config.apps_origin().is_some(), confirm).map_err(|err| ApiError::new(StatusCode::BAD_REQUEST, err))?;
                isolation_detail = Some(format!("isolation={mode}"));
                Some(mode)
            }
            None => None,
        };
        let registry = state.registry.current().map(|l| l.registry.clone());
        let conflicts = crate::sideload::type_conflicts(&read_manifest(&state.config.data_dir), registry.as_ref(), id, &extensions, claim);
        if body.extensions.is_some() && !conflicts.is_empty() && body.confirm_conflicts != Some(true) {
            let official = conflicts.iter().any(|c| c.official);
            let message = if official {
                "this file type is already opened by an official app; confirm and, to make this the default, tick that option"
            } else {
                "this file type is already opened by another app; confirm to continue"
            };
            return Err(ApiError::with_conflicts(StatusCode::CONFLICT, message, "type_conflict", conflicts));
        }
        let only_isolation = isolation_detail.is_some() && body.extensions.is_none() && body.grants.is_none() && body.claim_defaults.is_none() && body.auto_update.is_none();
        apps::set_sideload_meta(&state.config.data_dir, id, body.extensions.map(|_| extensions), body.grants.map(|_| grants), body.claim_defaults, isolation, registry.as_ref()).map_err(|err| ApiError::new(StatusCode::BAD_REQUEST, err))?;
        isolation_only = only_isolation;
    }
    let action = if body.disabled == Some(true) {
        "disable"
    } else if body.disabled == Some(false) {
        "enable"
    } else if isolation_only {
        "isolation"
    } else {
        "settings"
    };
    let after = read_manifest(&state.config.data_dir).apps.get(id).cloned();
    audit(state, &user.username, action, id, after.as_ref().map(|r| r.version.as_str()), after.as_ref().map(|r| r.sha256.as_str()), None, isolation_detail.as_deref());
    let _ = before;
    Ok(())
}

async fn start_source_install(state: AppState, actor: &str, id: String, version: Option<String>) -> Result<(StatusCode, Json<AdminApps>), ApiError> {
    let row = read_manifest(&state.config.data_dir).apps.get(&id).cloned().ok_or_else(|| ApiError::new(StatusCode::NOT_FOUND, "that app is not installed"))?;
    if row.sideload.as_ref().and_then(|m| m.source.as_ref()).is_none() {
        return Err(ApiError::new(StatusCode::BAD_REQUEST, "this app has no source to update from"));
    }
    {
        let jobs = state.app_jobs.lock().unwrap_or_else(|e| e.into_inner());
        if jobs.get(&id).is_some_and(|job| !job.done) {
            return Err(ApiError::new(StatusCode::CONFLICT, "this app is already being installed"));
        }
    }
    let started = unix_now();
    set_job(&state, &id, AppJob { action: "update".into(), version: version.clone(), phase: "starting".into(), percent: None, error: None, done: false, started });
    let task_state = state.clone();
    let task_id = id.clone();
    let actor = actor.to_string();
    let registry = state.registry.current().map(|l| l.registry.clone());
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
        let result = apps::update_from_source(&task_state.config.data_dir, &task_id, version.as_deref(), registry.as_ref(), progress).await;
        let mut jobs = task_state.app_jobs.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(job) = jobs.get_mut(&task_id) {
            job.done = true;
            job.percent = None;
            match result {
                Ok((version, sha)) => {
                    job.phase = "done".into();
                    job.version = Some(version.clone());
                    drop(jobs);
                    audit(&task_state, &actor, "update", &task_id, Some(&version), Some(&sha), Some("source"), None);
                }
                Err(err) => {
                    job.phase = "failed".into();
                    job.error = Some(err);
                }
            }
        }
    });
    Ok((StatusCode::ACCEPTED, Json(admin_view(&state))))
}

/// `POST /api/admin/apps/sideload`: upload a zip or install the first version from a source.
pub async fn admin_sideload(State(state): State<AppState>, AuthUser(user): AuthUser, multipart: axum::extract::Multipart) -> Result<Json<AdminApps>, ApiError> {
    require_super(&user)?;
    let (req, zip_path) = read_sideload_upload(&state, multipart).await?;
    if !req.confirm {
        return Err(ApiError::new(StatusCode::BAD_REQUEST, "confirm that this build is not on the artcraftCloud trust list"));
    }
    let id = crate::sideload::normalize_id(&req.id).map_err(|err| ApiError::new(StatusCode::BAD_REQUEST, err))?;
    let version = crate::sideload::normalize_version(&req.version).map_err(|err| ApiError::new(StatusCode::BAD_REQUEST, err))?;
    let extensions = crate::sideload::normalize_extensions(&req.extensions).map_err(|err| ApiError::new(StatusCode::BAD_REQUEST, err))?;
    let grants = crate::sideload::normalize_grants(&req.grants, req.bridge).map_err(|err| ApiError::new(StatusCode::BAD_REQUEST, err))?;
    let isolation = crate::sideload::normalize_isolation(
        Some(req.isolation.as_str()).filter(|s| !s.is_empty()),
        state.config.apps_origin().is_some(),
        req.confirm_regular,
    )
    .map_err(|err| ApiError::new(StatusCode::BAD_REQUEST, err))?;
    let source = crate::sideload::normalize_source(req.source.as_ref()).map_err(|err| ApiError::new(StatusCode::BAD_REQUEST, err))?;
    let registry = state.registry.current().map(|l| l.registry.clone());
    let manifest = read_manifest(&state.config.data_dir);
    if (manifest.apps.get(&id).is_some_and(|row| !row.is_sideload()) || registry.as_ref().is_some_and(|r| r.app(&id).is_some()))
        && !req.confirm_conflicts
    {
        let title = registry.as_ref().and_then(|r| r.app(&id)).map(|a| a.title.clone()).unwrap_or_else(|| id.clone());
        return Err(ApiError::with_conflicts(
            StatusCode::CONFLICT,
            format!("{title} is on the official app registry; replacing it with a sideloaded build needs an extra confirmation"),
            "official_overwrite",
            vec![TypeConflict { ext: String::new(), app: id, title, official: true }],
        ));
    }
    let conflicts = crate::sideload::type_conflicts(&manifest, registry.as_ref(), &id, &extensions, req.claim_defaults);
    if !conflicts.is_empty() && !req.confirm_conflicts {
        let official = conflicts.iter().any(|c| c.official);
        let message = if official {
            "this file type is already opened by an official app; confirm and, to make this the default, tick that option"
        } else {
            "this file type is already opened by another app; confirm to continue"
        };
        return Err(ApiError::with_conflicts(StatusCode::CONFLICT, message, "type_conflict", conflicts));
    }
    let meta = crate::sideload::meta(&req.name, &req.publisher, &req.blurb, req.bridge, extensions, grants, req.claim_defaults, source.clone(), isolation.clone()).map_err(|err| ApiError::new(StatusCode::BAD_REQUEST, err))?;
    let zip = match zip_path {
        Some(path) => path,
        None => {
            let source = source.clone().ok_or_else(|| ApiError::new(StatusCode::BAD_REQUEST, "upload a zip or connect a source"))?;
            download_first_version(&state, &id, &source, req.version.as_str()).await?
        }
    };
    let sha = tokio::task::spawn_blocking({
        let data = state.config.data_dir.clone();
        let meta = meta.clone();
        let zip = zip.clone();
        let id = id.clone();
        let version = version.clone();
        let registry = registry.clone();
        move || apps::install_sideload(&data, &id, &version, &zip, meta, registry.as_ref())
    })
    .await
    .map_err(|err| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, err.to_string()))?
    .map_err(|err| ApiError::new(StatusCode::BAD_REQUEST, err))?;
    let _ = tokio::fs::remove_file(&zip).await;
    let src = source.as_ref().map(|s| format!("{}:{}", s.kind, s.url));
    audit(&state, &user.username, "install", &id, Some(&version), Some(&sha), src.as_deref(), Some(&format!("sideload,isolation={isolation}")));
    Ok(Json(admin_view(&state)))
}

async fn read_sideload_upload(state: &AppState, mut multipart: axum::extract::Multipart) -> Result<(SideloadRequest, Option<PathBuf>), ApiError> {
    let mut meta: Option<SideloadRequest> = None;
    let mut zip_path = None;
    while let Some(field) = multipart.next_field().await.map_err(|err| ApiError::new(StatusCode::BAD_REQUEST, err.to_string()))? {
        let name = field.name().unwrap_or("").to_string();
        if name == "meta" {
            let bytes = field.bytes().await.map_err(|err| ApiError::new(StatusCode::BAD_REQUEST, err.to_string()))?;
            meta = Some(serde_json::from_slice(&bytes).map_err(|_| ApiError::new(StatusCode::BAD_REQUEST, "sideload metadata is not valid JSON"))?);
            continue;
        }
        if name == "file" || name == "zip" {
            let tmp = state.config.data_dir.join("tmp").join(format!("sideload-upload-{}", std::process::id()));
            if let Some(parent) = tmp.parent() {
                tokio::fs::create_dir_all(parent).await.map_err(|err| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, err.to_string()))?;
            }
            let bytes = field.bytes().await.map_err(|err| ApiError::new(StatusCode::BAD_REQUEST, err.to_string()))?;
            if bytes.len() as u64 > state.config.max_upload_bytes {
                return Err(ApiError::new(StatusCode::PAYLOAD_TOO_LARGE, "the zip is larger than ARTCRAFT_MAX_UPLOAD_MB"));
            }
            if bytes.len() < 4 || &bytes[..2] != b"PK" {
                return Err(ApiError::new(StatusCode::BAD_REQUEST, "upload a zip file"));
            }
            tokio::fs::write(&tmp, &bytes).await.map_err(|err| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, err.to_string()))?;
            zip_path = Some(tmp);
        }
    }
    let meta = meta.ok_or_else(|| ApiError::new(StatusCode::BAD_REQUEST, "missing sideload metadata"))?;
    Ok((meta, zip_path))
}

async fn download_first_version(state: &AppState, id: &str, source: &crate::manifest::SideloadSource, requested: &str) -> Result<PathBuf, ApiError> {
    let found = crate::sideload::check_source(id, source).await.map_err(|err| ApiError::new(StatusCode::BAD_REQUEST, err))?;
    let target = if requested.trim().is_empty() || requested == "0.0.1" {
        crate::sideload::newest(&found)
    } else {
        found.iter().find(|v| v.version == requested)
    }
    .ok_or_else(|| ApiError::new(StatusCode::BAD_REQUEST, "the source lists no usable version"))?;
    let tmp = state.config.data_dir.join("tmp").join(format!("sideload-src-{id}-{}", std::process::id()));
    if let Some(parent) = tmp.parent() {
        tokio::fs::create_dir_all(parent).await.map_err(|err| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, err.to_string()))?;
    }
    let client = reqwest::Client::builder()
        .user_agent(crate::registry::USER_AGENT)
        .timeout(std::time::Duration::from_secs(60))
        .build()
        .map_err(|err| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, err.to_string()))?;
    let response = client.get(&target.url).send().await.map_err(|err| ApiError::new(StatusCode::BAD_REQUEST, err.to_string()))?;
    if !response.status().is_success() {
        return Err(ApiError::new(StatusCode::BAD_REQUEST, format!("source zip returned {}", response.status())));
    }
    let bytes = response.bytes().await.map_err(|err| ApiError::new(StatusCode::BAD_REQUEST, err.to_string()))?;
    if bytes.len() as u64 > state.config.max_upload_bytes {
        return Err(ApiError::new(StatusCode::PAYLOAD_TOO_LARGE, "the zip is larger than ARTCRAFT_MAX_UPLOAD_MB"));
    }
    let sha = hex::encode(<sha2::Sha256 as sha2::Digest>::digest(&bytes));
    if !sha.eq_ignore_ascii_case(&target.sha256) {
        return Err(ApiError::new(StatusCode::BAD_REQUEST, "the downloaded zip does not match the source's sha256"));
    }
    tokio::fs::write(&tmp, &bytes).await.map_err(|err| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, err.to_string()))?;
    Ok(tmp)
}

/// `POST /api/admin/apps/{id}/source`: connect or clear a version source.
pub async fn admin_source(State(state): State<AppState>, AuthUser(user): AuthUser, Path(id): Path<String>, Json(body): Json<Option<AppSource>>) -> Result<Json<AdminApps>, ApiError> {
    require_super(&user)?;
    let source = crate::sideload::normalize_source(body.as_ref()).map_err(|err| ApiError::new(StatusCode::BAD_REQUEST, err))?;
    apps::set_sideload_source(&state.config.data_dir, &id, source.clone()).map_err(|err| ApiError::new(StatusCode::BAD_REQUEST, err))?;
    let url = source.as_ref().map(|s| s.url.clone());
    audit(&state, &user.username, "source", &id, None, None, url.as_deref(), None);
    Ok(Json(admin_view(&state)))
}

/// `POST /api/admin/apps/{id}/check`: look at a connected source for newer versions.
pub async fn admin_check(State(state): State<AppState>, AuthUser(user): AuthUser, Path(id): Path<String>) -> Result<Json<AdminApps>, ApiError> {
    require_super(&user)?;
    let row = read_manifest(&state.config.data_dir).apps.get(&id).cloned().ok_or_else(|| ApiError::new(StatusCode::NOT_FOUND, "that app is not installed"))?;
    let source = row.sideload.as_ref().and_then(|m| m.source.clone()).ok_or_else(|| ApiError::new(StatusCode::BAD_REQUEST, "this app has no source to check"))?;
    let found = crate::sideload::check_source(&id, &source).await.map_err(|err| ApiError::new(StatusCode::BAD_REQUEST, err))?;
    if let Some(newest) = crate::sideload::newest(&found) {
        apps::record_offered(&state.config.data_dir, &id, crate::sideload::offered_from(newest)).map_err(|err| ApiError::new(StatusCode::BAD_REQUEST, err))?;
    }
    Ok(Json(admin_view(&state)))
}

/// `POST /api/apps/{id}/ack`: the person accepted the first-open warning for an unverified app.
pub async fn ack_unverified(State(state): State<AppState>, AuthUser(user): AuthUser, Path(id): Path<String>) -> Result<StatusCode, ApiError> {
    if !artcraft_cloud_types::registry::valid_id(&id) {
        return Err(ApiError::new(StatusCode::BAD_REQUEST, "bad app id"));
    }
    state.db.ack_unverified_app(user.id, &id).map_err(ApiError::from)?;
    Ok(StatusCode::NO_CONTENT)
}

/// `POST /api/apps/{id}/capability`: the shell issues a short-lived token for a sideloaded app.
pub async fn issue_capability(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(id): Path<String>,
    Json(body): Json<crate::capability::IssueRequest>,
) -> Result<Json<crate::capability::IssueResponse>, ApiError> {
    if !artcraft_cloud_types::registry::valid_id(&id) {
        return Err(ApiError::new(StatusCode::BAD_REQUEST, "bad app id"));
    }
    let manifest = read_manifest(&state.config.data_dir);
    let Some(row) = manifest.apps.get(&id) else {
        return Err(ApiError::new(StatusCode::NOT_FOUND, "app is not installed"));
    };
    if row.disabled.is_some() {
        return Err(ApiError::new(StatusCode::FORBIDDEN, "app is disabled"));
    }
    let path = if body.create && body.path.is_none() {
        None
    } else {
        let raw = body.path.as_deref().unwrap_or("").trim();
        if raw.is_empty() {
            return Err(ApiError::new(StatusCode::BAD_REQUEST, "give the opened file path, or ask for a create-only token"));
        }
        let rel = crate::paths::RelPath::parse(raw).map_err(ApiError::from)?;
        if rel.is_root() {
            return Err(ApiError::new(StatusCode::BAD_REQUEST, "give the opened file path, or ask for a create-only token"));
        }
        let store = state.store.clone();
        let owner = user.id;
        let rel2 = rel.clone();
        blocking(move || store.open_path(owner, &rel2)).await?;
        Some(rel.as_str().to_string())
    };
    Ok(Json(state.capabilities.issue(user.id, &id, path)))
}

async fn check_sideload_sources(state: &AppState) {
    let manifest = read_manifest(&state.config.data_dir);
    for (id, row) in manifest.apps {
        let Some(source) = row.sideload.as_ref().and_then(|m| m.source.clone()) else { continue };
        if row.disabled.is_some() {
            continue;
        }
        match crate::sideload::check_source(&id, &source).await {
            Ok(found) => {
                if let Some(newest) = crate::sideload::newest(&found) {
                    let _ = apps::record_offered(&state.config.data_dir, &id, crate::sideload::offered_from(newest));
                }
            }
            Err(err) => tracing::warn!(app = id, "source check: {err}"),
        }
    }
}
