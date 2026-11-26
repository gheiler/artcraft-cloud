//! Apps (super admin): what the signed app registry offers, what is installed, and adding,
//! updating, rolling back or removing an app while the server runs. The server does the work and
//! checks every version against the registry; this page shows it and polls running installs.

use artcraft_cloud_types::{AdminApp, AdminApps, AppAlert, AppJob, AppSource, SideloadRequest};
use leptos::prelude::*;
use wasm_bindgen::{closure::Closure, JsCast};

use crate::api;
use crate::i18n::{t, tf};
use crate::icons::{app_icon, icon, Icon};
use crate::modal::Dialog;
use crate::route::Route;
use crate::ui::{ui, Ui};

/// Read the admin view now (alerts included). Super admins only.
pub fn load_alerts(ui: Ui) {
    wasm_bindgen_futures::spawn_local(async move {
        if let Ok(view) = api::get_json::<AdminApps>("/api/admin/apps").await {
            let _ = ui.app_alerts.try_set(view.alerts);
        }
    });
}

/// One sentence for an alert, in the reader's language.
pub fn alert_text(alert: &AppAlert) -> String {
    let args = [
        ("app", alert.title.as_str()),
        ("version", alert.version.as_str()),
        ("reason", alert.reason.as_str()),
        ("to", alert.to.as_deref().unwrap_or("")),
    ];
    match alert.action.as_deref() {
        Some("rolled_back") => tf("{app} {version} is blocked by the app registry ({reason}). It was rolled back to {to} automatically.", &args),
        Some("disabled") => tf("{app} {version} is blocked by the app registry ({reason}). It was disabled because no allowed earlier version is on this server.", &args),
        _ => tf("{app} {version} is blocked by the app registry ({reason}). Update it or roll it back.", &args),
    }
}

/// The banner every page shows the super admin while an installed app is blocked.
#[component]
pub fn AppAlerts() -> impl IntoView {
    let ui = ui();
    let shown = move || {
        ui.user.with(|u| u.as_ref().is_some_and(|u| u.super_admin))
            && !ui.app_alerts.with(Vec::is_empty)
            && !matches!(ui.route.get(), Route::Apps)
    };
    view! {
        <Show when=shown>
            <div class="app-alerts" role="alert">
                {icon(Icon::Shield)}
                <div class="app-alerts-text">
                    {move || ui.app_alerts.get().iter().map(|a| view! { <p>{alert_text(a)}</p> }).collect_view()}
                </div>
                <button type="button" class="button small" on:click=move |_| ui.go(Route::Apps)>{t("Open Apps")}</button>
            </div>
        </Show>
    }
}

fn after(ms: i32, run: impl FnOnce() + 'static) {
    let callback = Closure::once_into_js(run);
    if let Some(window) = web_sys::window() {
        let _ = window
            .set_timeout_with_callback_and_timeout_and_arguments_0(callback.unchecked_ref(), ms);
    }
}

fn when(seconds: Option<i64>) -> String {
    match seconds {
        Some(s) if s > 0 => {
            let date = js_sys::Date::new(&wasm_bindgen::JsValue::from_f64(s as f64 * 1000.0));
            String::from(
                date.to_locale_string(crate::i18n::language(), &wasm_bindgen::JsValue::UNDEFINED),
            )
        }
        _ => t("never").to_string(),
    }
}

fn running(view: &AdminApps) -> bool {
    view.apps
        .iter()
        .any(|a| a.job.as_ref().is_some_and(|j| !j.done))
}

fn confirm(message: &str) -> bool {
    web_sys::window()
        .and_then(|w| w.confirm_with_message(message).ok())
        .unwrap_or(false)
}

fn state_label(state: &str) -> String {
    match state {
        "ok" => t("Allowed").to_string(),
        "blocked" => t("Blocked").to_string(),
        "incompatible" => t("Incompatible").to_string(),
        "unlisted" => t("Not in the registry").to_string(),
        _ => t("Not installed").to_string(),
    }
}

fn signature_label(signature: &str) -> String {
    match signature {
        "verified" | "openpgp" | "ed25519" => t("Signed by the publisher").to_string(),
        "source" => t("Checksum from the app's source").to_string(),
        "registry" | "none" => t("Checksum from the registry").to_string(),
        "local" => t("Installed by hand").to_string(),
        _ => t("Unknown").to_string(),
    }
}

fn phase_label(job: &AppJob) -> String {
    match job.phase.as_str() {
        "starting" => t("Starting…").to_string(),
        "downloading" => match job.percent {
            Some(p) => tf("Downloading… {percent}%", &[("percent", &p.to_string())]),
            None => t("Downloading…").to_string(),
        },
        "verifying" => t("Checking signatures and checksums…").to_string(),
        "installing" => t("Installing…").to_string(),
        "done" => t("Done.").to_string(),
        "failed" => t("Failed.").to_string(),
        other => other.to_string(),
    }
}

#[component]
pub fn AppsPage() -> impl IntoView {
    let ui = ui();
    let data = RwSignal::new(None::<AdminApps>);
    // The app whose install dialog is open, and the one whose versions are listed.
    let job_for = RwSignal::new(None::<String>);
    let versions_for = RwSignal::new(None::<String>);
    let types_for = RwSignal::new(None::<String>);
    let grants_for = RwSignal::new(None::<String>);
    let source_for = RwSignal::new(None::<String>);
    let isolation_for = RwSignal::new(None::<String>);
    let refreshing = RwSignal::new(false);
    let polling = RwSignal::new(false);

    let reload_catalog = move || {
        wasm_bindgen_futures::spawn_local(async move {
            if let Ok(list) = api::apps().await {
                crate::catalog::adopt(ui, list);
            }
        });
    };
    let adopt = move |view: AdminApps| {
        let was_running = data.with_untracked(|d| d.as_ref().is_some_and(running));
        let now_running = running(&view);
        let _ = ui.app_alerts.try_set(view.alerts.clone());
        let _ = data.try_set(Some(view));
        if was_running && !now_running {
            reload_catalog();
        }
    };
    let load = move || {
        wasm_bindgen_futures::spawn_local(async move {
            match api::get_json::<AdminApps>("/api/admin/apps").await {
                Ok(view) => adopt(view),
                Err(err) => ui.say(err),
            }
        });
    };
    // While something installs, ask again every second.
    Effect::new(move |_| {
        let busy = data.with(|d| d.as_ref().is_some_and(running));
        if busy && !polling.get_untracked() {
            polling.set(true);
            after(1000, move || {
                let _ = polling.try_set(false);
                if data.try_with_untracked(|_| ()).is_some() {
                    load();
                }
            });
        }
    });
    load();

    let finish = move |result: Result<AdminApps, String>| match result {
        Ok(view) => {
            adopt(view);
            reload_catalog();
        }
        Err(err) => ui.say(err),
    };
    let install = move |id: String, version: Option<String>| {
        let _ = versions_for.try_set(None);
        let _ = job_for.try_set(Some(id.clone()));
        wasm_bindgen_futures::spawn_local(async move {
            match api::json::<AdminApps>(
                "POST",
                &format!("/api/admin/apps/{id}/install"),
                &serde_json::json!({ "version": version }),
            )
            .await
            {
                Ok(view) => adopt(view),
                Err(err) => {
                    let _ = job_for.try_set(None);
                    ui.say(err);
                }
            }
        });
    };
    let rollback = move |app: AdminApp| {
        let previous = app.previous.clone().unwrap_or_default();
        if !confirm(&tf(
            "Roll {app} back to {version}?",
            &[("app", &app.title), ("version", &previous)],
        )) {
            return;
        }
        wasm_bindgen_futures::spawn_local(async move {
            let result = api::json::<AdminApps>(
                "POST",
                &format!("/api/admin/apps/{}/rollback", app.id),
                &serde_json::json!({}),
            )
            .await;
            if result.is_ok() {
                ui.say(tf(
                    "{app} is back on {version}.",
                    &[("app", &app.title), ("version", &previous)],
                ));
            }
            finish(result);
        });
    };
    let remove = move |app: AdminApp| {
        if !confirm(&tf("Remove {app} from this server? Documents stay in the drive; it is not installed again until you add it.", &[("app", &app.title)])) {
            return;
        }
        wasm_bindgen_futures::spawn_local(async move {
            finish(
                api::json::<AdminApps>(
                    "DELETE",
                    &format!("/api/admin/apps/{}", app.id),
                    &serde_json::json!({}),
                )
                .await,
            );
        });
    };
    let settings = move |id: String, body: serde_json::Value| {
        wasm_bindgen_futures::spawn_local(async move {
            finish(
                api::json::<AdminApps>("PUT", &format!("/api/admin/apps/{id}/settings"), &body)
                    .await,
            );
        });
    };
    let dismiss = move |id: String| {
        wasm_bindgen_futures::spawn_local(async move {
            finish(
                api::json::<AdminApps>(
                    "POST",
                    &format!("/api/admin/apps/{id}/dismiss"),
                    &serde_json::json!({}),
                )
                .await,
            );
        });
    };
    let refresh = move |_| {
        refreshing.set(true);
        wasm_bindgen_futures::spawn_local(async move {
            let result =
                api::json::<AdminApps>("POST", "/api/admin/apps/refresh", &serde_json::json!({}))
                    .await;
            let _ = refreshing.try_set(false);
            match result {
                Ok(view) => {
                    match &view.registry.last_error {
                        Some(err) if view.registry.last_attempt > view.registry.last_fetch => ui
                            .say(tf(
                                "The registry could not be fetched: {error}",
                                &[("error", err)],
                            )),
                        _ => ui.say(t("The app registry is up to date.")),
                    }
                    adopt(view);
                    reload_catalog();
                }
                Err(err) => ui.say(err),
            }
        });
    };

    let app_of = move |id: &str| {
        data.with(|d| {
            d.as_ref()
                .and_then(|d| d.apps.iter().find(|a| a.id == id).cloned())
        })
    };
    let installed = move || {
        data.with(|d| {
            d.as_ref()
                .map(|d| {
                    d.apps
                        .iter()
                        .filter(|a| a.installed.is_some())
                        .cloned()
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default()
        })
    };
    let available = move || {
        data.with(|d| {
            d.as_ref()
                .map(|d| {
                    d.apps
                        .iter()
                        .filter(|a| a.installed.is_none() && a.in_registry)
                        .cloned()
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default()
        })
    };

    let danger_zone = move || {
        view! {
            <section class="card settings-card danger-zone" id="danger-zone">
                <h2>{t("Danger zone")}</h2>
                <p class="muted">{t("Install an app that is not on the artcraftCloud trust list. It runs in a sandbox on this site and cannot use the signed-in session. Only add software you trust.")}</p>
                <SideloadForm disabled=false apps_origin=data.with(|d| d.as_ref().and_then(|d| d.apps_origin.clone())) on_done=Callback::new(finish) />
            </section>
        }
    };
    let audit_log = move || {
        let events = data.with(|d| d.as_ref().map(|d| d.audit.clone()).unwrap_or_default());
        view! {
            <section class="card settings-card">
                <h2>{t("Audit log")}</h2>
                <Show when={ let events = events.clone(); move || !events.is_empty() } fallback=|| view! { <p class="empty">{t("No app installs, updates or removals have been recorded yet.")}</p> }>
                    <table class="audit-log">
                        <thead><tr><th>{t("When")}</th><th>{t("Who")}</th><th>{t("Action")}</th><th>{t("App")}</th><th>{t("Version")}</th><th>{t("Details")}</th></tr></thead>
                        <tbody>
                            {events.iter().map(|e| view! {
                                <tr>
                                    <td>{when(Some(e.at))}</td>
                                    <td>{e.actor.clone()}</td>
                                    <td>{audit_action(&e.action)}</td>
                                    <td>{e.app.clone()}</td>
                                    <td>{e.version.clone().unwrap_or_default()}</td>
                                    <td class="muted">{e.detail.clone().or(e.source.clone()).or(e.sha256.clone().map(|s| format!("sha256 {s}")))}</td>
                                </tr>
                            }).collect_view()}
                        </tbody>
                    </table>
                </Show>
            </section>
        }
    };
    let registry_card = move || {
        data.get().map(|d| {
            let r = d.registry;
            let source = match r.source.as_str() {
                "network" => t("Fetched from the registry"),
                "cache" => t("Saved copy of an earlier fetch"),
                "embedded" => t("Copy built into this artcraftCloud"),
                _ => t("No registry"),
            };
            view! {
                <section class="card settings-card registry-card">
                    <div class="files-head">
                        <h2>{t("App registry")}</h2>
                        <button type="button" class="button" disabled=move || refreshing.get() on:click=refresh>{move || if refreshing.get() { t("Checking…") } else { t("Check now") }}</button>
                    </div>
                    <dl class="facts">
                        <dt>{t("Signature")}</dt>
                        <dd>{if r.signature_ok { view! { <span class="badge primary">{icon(Icon::Shield)}{t("Verified")}</span> }.into_any() } else { view! { <span class="badge danger">{t("Not verified")}</span> }.into_any() }}
                            {r.custom_keys.then(|| view! { <span class="muted">" "{t("(your own registry keys)")}</span> })}</dd>
                        <dt>{t("In use")}</dt><dd>{source}" · "{tf("serial {serial}", &[("serial", &r.serial.to_string())])}</dd>
                        <dt>{t("Source")}</dt><dd><code>{r.url.clone()}</code></dd>
                        <dt>{t("Last fetched")}</dt><dd>{when(r.last_fetch)}</dd>
                        <dt>{t("Valid until")}</dt><dd class:bad=r.expired>{when(Some(r.expires))}{r.expired.then(|| view! { " · "<strong>{t("expired: new apps cannot be installed until it is renewed")}</strong> })}</dd>
                        {r.last_error.clone().filter(|_| r.last_attempt > r.last_fetch).map(|err| view! { <dt>{t("Last check")}</dt><dd class="bad">{err}</dd> })}
                    </dl>
                </section>
            }
        })
    };

    let alerts = move || {
        data.with(|d| d.as_ref().map(|d| d.alerts.clone()).unwrap_or_default())
            .into_iter()
            .map(|alert| {
                let id = alert.app.clone();
                let notice = alert.action.as_deref() == Some("rolled_back");
                view! {
                    <div class="app-alerts in-page" role="alert">
                        {icon(Icon::Shield)}
                        <div class="app-alerts-text">
                            <p>{alert.security.then(|| view! { <strong>{t("Security")}": "</strong> })}{alert_text(&alert)}</p>
                        </div>
                        {notice.then(|| view! { <button type="button" class="button small" on:click=move |_| dismiss(id.clone())>{t("Dismiss")}</button> })}
                    </div>
                }
            })
            .collect_view()
    };

    let installed_row = move |app: AdminApp| {
        let id = app.id.clone();
        let state = app.state.clone();
        let version = app.installed.clone().unwrap_or_default();
        let job = app.job.clone().filter(|j| !j.done);
        let (a1, a2, a3, a4, a5) = (
            app.clone(),
            app.clone(),
            app.clone(),
            app.clone(),
            app.clone(),
        );
        let disabled = app.disabled.is_some();
        view! {
            <li class="app-row" class:blocked=state == "blocked">
                {app_icon(&id, "app-row-icon")}
                <div class="app-row-text">
                    <strong>{app.title.clone()}" "<span class="muted">{version.clone()}</span></strong>
                    <span class="muted">{app.publisher.clone()}" · "{signature_label(app.signature.as_deref().unwrap_or(""))}</span>
                    <span class="app-row-badges">
                        {app.unverified.then(|| view! { <span class="badge danger">{t("Unverified")}</span> })}
                        <span class="badge" class:danger=state == "blocked" || state == "incompatible">{state_label(&state)}</span>
                        {app.reason.clone().map(|r| view! { <span class="muted">{r}</span> })}
                        {disabled.then(|| view! { <span class="badge danger">{t("Disabled")}</span> })}
                        {app.update.clone().map(|v| view! { <span class="badge primary">{tf("Update available: {version}", &[("version", &v)])}</span> })}
                        {job.map(|j| view! { <span class="badge">{phase_label(&j)}</span> })}
                        {app.unverified.then(|| view! { <span class="badge">{isolation_label(&app.isolation)}</span> })}
                        {app.official_available.then(|| view! { <span class="badge">{t("Now in the official registry")}</span> })}
                    </span>
                </div>
                <div class="app-row-actions">
                    <label class="check" title=t("Install new allowed versions on its own (checked every 6 hours)")>
                        <input type="checkbox" prop:checked=app.auto_update disabled=!app.in_registry && app.source.is_none() on:change=move |ev| settings(a1.id.clone(), serde_json::json!({ "auto_update": event_target_checked(&ev) })) />
                        <span>{t("Auto-update")}</span>
                    </label>
                    {app.update.clone().map(|v| { let id = id.clone(); view! { <button type="button" class="button primary small" on:click=move |_| install(id.clone(), Some(v.clone()))>{t("Update")}</button> } })}
                    {(app.in_registry || app.source.is_some()).then(|| view! { <button type="button" class="button small" on:click=move |_| versions_for.set(Some(a2.id.clone()))>{t("Versions…")}</button> })}
                    {app.official_available.then(|| {
                        let id = id.clone();
                        view! { <button type="button" class="button small primary" on:click=move |_| {
                            if confirm(t("Switch this install to the official, trusted build from the app registry?")) {
                                install(id.clone(), None);
                            }
                        }>{t("Switch to official")}</button> }
                    })}
                    {app.unverified.then(|| {
                        let id_types = id.clone();
                        let id_grants = id.clone();
                        let id_source = id.clone();
                        let id_isolation = id.clone();
                        view! {
                            <button type="button" class="button small" on:click=move |_| types_for.set(Some(id_types.clone()))>{t("File types…")}</button>
                            <button type="button" class="button small" on:click=move |_| grants_for.set(Some(id_grants.clone()))>{t("Grants…")}</button>
                            <button type="button" class="button small" on:click=move |_| isolation_for.set(Some(id_isolation.clone()))>{t("Isolation…")}</button>
                            <button type="button" class="button small" on:click=move |_| source_for.set(Some(id_source.clone()))>{t("Source…")}</button>
                        }
                    })}
                    {app.can_roll_back.then(|| view! { <button type="button" class="button small" on:click=move |_| rollback(a3.clone())>{tf("Roll back to {version}", &[("version", a3.previous.as_deref().unwrap_or(""))])}</button> })}
                    <button type="button" class="button small" disabled=state == "blocked" && disabled on:click=move |_| settings(a4.id.clone(), serde_json::json!({ "disabled": a4.disabled.is_none() }))>{if disabled { t("Enable") } else { t("Disable") }}</button>
                    <button type="button" class="button small danger" on:click=move |_| remove(a5.clone())>{t("Remove")}</button>
                </div>
            </li>
        }
    };

    let available_row = move |app: AdminApp| {
        let id = app.id.clone();
        let id2 = app.id.clone();
        let job = app.job.clone().filter(|j| !j.done);
        let target = app.recommended.clone();
        view! {
            <li class="app-row">
                {app_icon(&id, "app-row-icon")}
                <div class="app-row-text">
                    <strong>{app.title.clone()}</strong>
                    <span class="muted">{app.blurb.clone()}" · "{app.publisher.clone()}</span>
                    <span class="app-row-badges">
                        {target.clone().map(|v| view! { <span class="badge">{v}</span> })}
                        {job.map(|j| view! { <span class="badge">{phase_label(&j)}</span> })}
                        <a class="muted" href=app.repo_url.clone() target="_blank" rel="noopener">{t("Source code")}</a>
                    </span>
                </div>
                <div class="app-row-actions">
                    <button type="button" class="button small" on:click=move |_| versions_for.set(Some(id2.clone()))>{t("Versions…")}</button>
                    <button type="button" class="button primary small" disabled=target.is_none() on:click=move |_| install(id.clone(), None)>{icon(Icon::Plus)}<span>{t("Add")}</span></button>
                </div>
            </li>
        }
    };

    let job_dialog = move || {
        let id = job_for.get()?;
        let app = app_of(&id)?;
        let job = app.job.clone();
        let close = Callback::new(move |_| job_for.set(None));
        let title = match job.as_ref().map(|j| j.action.as_str()) {
            Some("add") => tf("Adding {app}", &[("app", &app.title)]),
            Some("downgrade") => tf("Changing {app}", &[("app", &app.title)]),
            _ => tf("Updating {app}", &[("app", &app.title)]),
        };
        let done = job.as_ref().is_none_or(|j| j.done);
        let failed = job.as_ref().and_then(|j| j.error.clone());
        Some(view! {
            <Dialog title=title on_close=close>
                <div class="dialog-body">
                    {job.clone().map(|j| view! {
                        <p>{tf("Version {version}", &[("version", j.version.as_deref().unwrap_or(""))])}</p>
                        <div class="progress" class:indeterminate=j.percent.is_none() && !j.done>
                            <div class="progress-bar" style=format!("width: {}%", if j.done { 100 } else { j.percent.unwrap_or(30) })></div>
                        </div>
                        <p class="muted" role="status">{phase_label(&j)}</p>
                    })}
                    {failed.map(|err| view! { <p class="test-result bad" role="alert">{err}</p> })}
                    {(done && app.job.as_ref().is_some_and(|j| j.error.is_none())).then(|| view! {
                        <p class="test-result ok">{tf("{app} {version} is installed. People see it the next time their page loads.", &[("app", &app.title), ("version", app.installed.as_deref().unwrap_or(""))])}</p>
                    })}
                    <footer>
                        <button type="button" class="button" class:primary=done on:click=move |_| job_for.set(None)>{if done { t("Close") } else { t("Hide") }}</button>
                    </footer>
                </div>
            </Dialog>
        })
    };

    let versions_dialog = move || {
        let id = versions_for.get()?;
        let app = app_of(&id)?;
        let close = Callback::new(move |_| versions_for.set(None));
        let installed = app.installed.clone();
        Some(view! {
            <Dialog title=tf("{app} versions", &[("app", &app.title)]) wide=true on_close=close>
                <div class="dialog-body">
                    <div class="files-head">
                        <p class="muted">{if app.unverified { t("Versions from the connected source. Updates stay off until you turn auto-update on.") } else { t("Only versions the app registry allows can be installed. Each is checked against the registry's checksum and the publisher's signature.") }}</p>
                        {app.source.is_some().then(|| {
                            let id = id.clone();
                            view! { <button type="button" class="button small" on:click=move |_| {
                                let id = id.clone();
                                wasm_bindgen_futures::spawn_local(async move {
                                    finish(api::json::<AdminApps>("POST", &format!("/api/admin/apps/{id}/check"), &serde_json::json!({})).await);
                                });
                            }>{t("Check for versions")}</button> }
                        })}
                    </div>
                    <ul class="version-list">
                        {app.versions.iter().map(|v| {
                            let current = installed.as_deref() == Some(v.version.as_str());
                            let id = id.clone();
                            let version = v.version.clone();
                            view! {
                                <li class="version-row">
                                    <strong>{v.version.clone()}</strong>
                                    {v.recommended.then(|| view! { <span class="badge primary">{t("Recommended")}</span> })}
                                    {current.then(|| view! { <span class="badge">{t("Installed")}</span> })}
                                    <span class="badge" class:danger=v.status != "ok">{state_label(&v.status)}</span>
                                    <span class="muted">{v.reason.clone().unwrap_or_else(|| signature_label(&v.signed))}</span>
                                    <span class="grow"></span>
                                    {(v.installable && !current).then(|| view! {
                                        <button type="button" class="button small" on:click=move |_| install(id.clone(), Some(version.clone()))>{t("Install")}</button>
                                    })}
                                </li>
                            }
                        }).collect_view()}
                    </ul>
                </div>
            </Dialog>
        })
    };

    view! {
        <section class="drive settings-page apps-page">
            <header class="toolbar"><nav class="crumbs"><span class="crumb">{t("Apps")}</span></nav></header>
            <p class="muted intro">{t("Apps come from the artcraftCloud app registry, a signed list of apps and the versions that are safe to run. Add or update an app here; there is no need to update this server.")}</p>
            {alerts}
            <Show when=move || data.with(Option::is_some) fallback=|| view! { <div class="boot"><span class="spinner"></span></div> }>
                <section class="card settings-card">
                    <h2>{t("Installed")}</h2>
                    <ul class="app-list">{move || installed().into_iter().map(installed_row).collect_view()}</ul>
                </section>
                <section class="card settings-card">
                    <h2>{t("Available")}</h2>
                    <Show when=move || !available().is_empty() fallback=|| view! { <p class="empty">{t("Every app in the registry is installed.")}</p> }>
                        <ul class="app-list">{move || available().into_iter().map(available_row).collect_view()}</ul>
                    </Show>
                </section>
                {registry_card}
                {danger_zone}
                {audit_log}
            </Show>
            {job_dialog}
            {versions_dialog}
            {move || types_for.get().and_then(|id| app_of(&id)).map(|app| view! {
                <FileTypesDialog app=app on_done=Callback::new(finish) on_close=Callback::new(move |_| types_for.set(None)) />
            })}
            {move || grants_for.get().and_then(|id| app_of(&id)).map(|app| view! {
                <GrantsDialog app=app on_done=Callback::new(finish) on_close=Callback::new(move |_| grants_for.set(None)) />
            })}
            {move || isolation_for.get().and_then(|id| app_of(&id)).map(|app| view! {
                <IsolationDialog app=app apps_origin=data.with(|d| d.as_ref().and_then(|d| d.apps_origin.clone())) on_done=Callback::new(finish) on_close=Callback::new(move |_| isolation_for.set(None)) />
            })}
            {move || source_for.get().and_then(|id| app_of(&id)).map(|app| view! {
                <SourceDialog app=app on_done=Callback::new(finish) on_close=Callback::new(move |_| source_for.set(None)) />
            })}
        </section>
    }
}

fn audit_action(action: &str) -> String {
    match action {
        "install" => t("Installed").to_string(),
        "update" => t("Updated").to_string(),
        "rollback" => t("Rolled back").to_string(),
        "disable" => t("Disabled").to_string(),
        "enable" => t("Enabled").to_string(),
        "remove" => t("Removed").to_string(),
        "settings" => t("Settings changed").to_string(),
        "source" => t("Source changed").to_string(),
        "isolation" => t("Isolation changed").to_string(),
        other => other.to_string(),
    }
}

fn parse_exts(raw: &str) -> Vec<String> {
    raw.split([',', ' ', ';'])
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

#[component]
fn SideloadForm(
    disabled: bool,
    apps_origin: Option<String>,
    on_done: Callback<Result<AdminApps, String>>,
) -> impl IntoView {
    let id = RwSignal::new(String::new());
    let name = RwSignal::new(String::new());
    let version = RwSignal::new(String::from("0.1.0"));
    let publisher = RwSignal::new(String::new());
    let blurb = RwSignal::new(String::new());
    let extensions = RwSignal::new(String::new());
    let source_kind = RwSignal::new(String::from("none"));
    let source_url = RwSignal::new(String::new());
    let bridge = RwSignal::new(true);
    let confirm = RwSignal::new(false);
    let claim = RwSignal::new(false);
    let grant_drive = RwSignal::new(false);
    let grant_clipboard = RwSignal::new(false);
    let grant_fullscreen = RwSignal::new(false);
    let isolation = RwSignal::new(String::from("sandbox"));
    let confirm_regular = RwSignal::new(false);
    let busy = RwSignal::new(false);
    let file = RwSignal::new(None::<web_sys::File>);
    let error = RwSignal::new(String::new());
    let confirm_conflicts = RwSignal::new(false);
    view! {
        <form class="sideload-form" on:submit=move |ev| {
            ev.prevent_default();
            if disabled || busy.get_untracked() {
                return;
            }
            error.set(String::new());
            let mut grants: Vec<String> = Vec::new();
            if bridge.get_untracked() {
                grants.push("document".into());
                if grant_drive.get_untracked() { grants.push("drive".into()); }
                if grant_clipboard.get_untracked() { grants.push("clipboard".into()); }
                if grant_fullscreen.get_untracked() { grants.push("fullscreen".into()); }
            } else {
                if grant_clipboard.get_untracked() { grants.push("clipboard".into()); }
                if grant_fullscreen.get_untracked() { grants.push("fullscreen".into()); }
            }
            let meta = SideloadRequest {
                id: id.get_untracked(),
                name: name.get_untracked(),
                version: version.get_untracked(),
                publisher: publisher.get_untracked(),
                blurb: blurb.get_untracked(),
                bridge: bridge.get_untracked(),
                extensions: parse_exts(&extensions.get_untracked()),
                grants,
                confirm: confirm.get_untracked(),
                confirm_conflicts: false,
                claim_defaults: claim.get_untracked(),
                isolation: isolation.get_untracked(),
                confirm_regular: confirm_regular.get_untracked(),
                source: match source_kind.get_untracked().as_str() {
                    "github" | "manifest" => Some(AppSource { kind: source_kind.get_untracked(), url: source_url.get_untracked() }),
                    _ => None,
                },
            };
            if !meta.confirm {
                error.set(t("Tick the box to confirm you understand this app is not on the trust list.").to_string());
                return;
            }
            let zip = file.get_untracked();
            if zip.is_none() && meta.source.is_none() {
                error.set(t("Upload a zip or connect a source.").to_string());
                return;
            }
            busy.set(true);
            wasm_bindgen_futures::spawn_local(async move {
                let send = |conflicts: bool| {
                    let mut meta = meta.clone();
                    meta.confirm_conflicts = conflicts;
                    let form = web_sys::FormData::new().expect("form");
                    let _ = form.append_with_str("meta", &serde_json::to_string(&meta).unwrap_or_default());
                    if let Some(zip) = &zip {
                        let _ = form.append_with_blob_and_filename("file", zip, &zip.name());
                    }
                    async move { crate::api::post_form::<AdminApps>("/api/admin/apps/sideload", form).await }
                };
                let result = send(confirm_conflicts.get_untracked()).await;
                let result = match result {
                    Err(msg) if msg.contains("already opened") || msg.contains("official") => {
                        error.set(format!("{msg} {}", t("Tick “continue anyway” below if you still want to install, then try again.")));
                        confirm_conflicts.set(true);
                        Err(msg)
                    }
                    other => other,
                };
                busy.set(false);
                match result {
                    Ok(view) => on_done.run(Ok(view)),
                    Err(err) => {
                        if error.get_untracked().is_empty() {
                            error.set(err.clone());
                        }
                        on_done.run(Err(err));
                    }
                }
            });
        }>
            <div class="sideload-grid">
                <label class="stack"><span>{t("App id")}</span>
                    <input class="field" prop:value=move || id.get() on:input=move |ev| id.set(event_target_value(&ev)) placeholder="myapp" disabled=disabled />
                    <span class="muted">{t("Lower-case letters, numbers and dashes. This becomes the folder name under /apps.")}</span>
                </label>
                <label class="stack"><span>{t("Name")}</span>
                    <input class="field" prop:value=move || name.get() on:input=move |ev| name.set(event_target_value(&ev)) disabled=disabled />
                </label>
                <label class="stack"><span>{t("Version")}</span>
                    <input class="field" prop:value=move || version.get() on:input=move |ev| version.set(event_target_value(&ev)) disabled=disabled />
                </label>
                <label class="stack"><span>{t("Publisher")}</span>
                    <input class="field" prop:value=move || publisher.get() on:input=move |ev| publisher.set(event_target_value(&ev)) disabled=disabled />
                </label>
                <label class="stack wide"><span>{t("Description")}</span>
                    <input class="field" prop:value=move || blurb.get() on:input=move |ev| blurb.set(event_target_value(&ev)) disabled=disabled />
                </label>
                <label class="stack wide"><span>{t("File types")}</span>
                    <input class="field" prop:value=move || extensions.get() on:input=move |ev| extensions.set(event_target_value(&ev)) placeholder="md, txt" disabled=disabled />
                    <span class="muted">{t("Comma-separated suffixes this app should open, for example md,txt. Not taken from the zip.")}</span>
                </label>
                <label class="stack"><span>{t("Source")}</span>
                    <select class="field" prop:value=move || source_kind.get() on:change=move |ev| source_kind.set(event_target_value(&ev)) disabled=disabled>
                        <option value="none">{t("None (zip only)")}</option>
                        <option value="github">{t("GitHub releases")}</option>
                        <option value="manifest">{t("Manifest URL")}</option>
                    </select>
                </label>
                <label class="stack"><span>{t("Source URL or owner/repo")}</span>
                    <input class="field" prop:value=move || source_url.get() on:input=move |ev| source_url.set(event_target_value(&ev)) disabled=disabled || source_kind.get() == "none" placeholder="owner/repo or https://…" />
                    <span class="muted">{t("A GitHub owner/repo with the usual release zips, or an https URL to a manifest.")}</span>
                </label>
                <label class="stack wide"><span>{t("Zip file")}</span>
                    <input class="field" type="file" accept=".zip,application/zip" disabled=disabled on:change=move |ev| {
                        let input = event_target::<web_sys::HtmlInputElement>(&ev);
                        let next = input.and_then(|el| el.files()).and_then(|list| list.get(0));
                        file.set(next);
                    } />
                </label>
            </div>
            <label class="check"><input type="checkbox" prop:checked=move || claim.get() on:change=move |ev| claim.set(event_target_checked(&ev)) disabled=disabled /><span>{t("Make it the default app for these file types, even if an official app already opens them")}</span></label>
            <fieldset class="grant-list" disabled=disabled>
                <legend>{t("What this app may do")}</legend>
                <label class="check"><input type="checkbox" prop:checked=move || bridge.get() on:change=move |ev| bridge.set(event_target_checked(&ev)) disabled=disabled /><span><strong>{t("Host bridge")}</strong><span class="muted">{t("The app opens and saves the document you chose, and can create a new file only where you pick in the save dialog. It does not get the rest of your drive unless you also grant that below.")}</span></span></label>
                <label class="check"><input type="checkbox" prop:checked=true disabled=true /><span><strong>{t("Opened document")}</strong><span class="muted">{t("Always included for a host-bridge app: edit and save the file that was opened, and create a new file through the shell's save dialog.")}</span></span></label>
                <label class="check"><input type="checkbox" prop:checked=move || grant_drive.get() on:change=move |ev| grant_drive.set(event_target_checked(&ev)) disabled=move || disabled || !bridge.get() /><span><strong>{t("Other files on the drive")}</strong><span class="muted">{t("The app can open or insert other files you pick in the shell's file dialog (File ▸ Open, a picture…). It still cannot use your signed-in session. Only grant this if you trust the app with any file you might choose.")}</span></span></label>
                <label class="check"><input type="checkbox" prop:checked=move || grant_clipboard.get() on:change=move |ev| grant_clipboard.set(event_target_checked(&ev)) disabled=disabled /><span><strong>{t("Clipboard")}</strong><span class="muted">{t("Read from and write to the clipboard while the app is open.")}</span></span></label>
                <label class="check"><input type="checkbox" prop:checked=move || grant_fullscreen.get() on:change=move |ev| grant_fullscreen.set(event_target_checked(&ev)) disabled=disabled /><span><strong>{t("Full screen")}</strong><span class="muted">{t("Allow the app to enter full screen.")}</span></span></label>
            </fieldset>
            <IsolationPicker mode=isolation confirm_regular=confirm_regular apps_origin=apps_origin.clone() disabled=disabled />
            <label class="check confirm-risk"><input type="checkbox" prop:checked=move || confirm.get() on:change=move |ev| confirm.set(event_target_checked(&ev)) disabled=disabled /><span>{t("I understand this build is not on the artcraftCloud trust list and I trust the file I am adding.")}</span></label>
            <Show when=move || confirm_conflicts.get() || !error.get().is_empty()>
                <label class="check confirm-risk"><input type="checkbox" prop:checked=move || confirm_conflicts.get() on:change=move |ev| confirm_conflicts.set(event_target_checked(&ev)) disabled=disabled /><span>{t("Continue anyway")}</span></label>
            </Show>
            <Show when=move || !error.get().is_empty()><p class="form-error" role="alert">{move || error.get()}</p></Show>
            <button type="submit" class="button danger-fill" disabled=move || disabled || busy.get()>{move || if busy.get() { t("Installing…") } else { t("Install unverified app") }}</button>
        </form>
    }
}

#[component]
fn FileTypesDialog(
    app: AdminApp,
    on_done: Callback<Result<AdminApps, String>>,
    on_close: Callback<()>,
) -> impl IntoView {
    let id = app.id.clone();
    let extensions = RwSignal::new(app.extensions.join(", "));
    let claim = RwSignal::new(app.claim_defaults);
    let confirm_conflicts = RwSignal::new(false);
    let error = RwSignal::new(String::new());
    let busy = RwSignal::new(false);
    view! {
        <Dialog title=tf("File types for {app}", &[("app", &app.title)]) on_close=on_close>
            <form class="dialog-body" on:submit=move |ev| {
                ev.prevent_default();
                if busy.get_untracked() { return; }
                error.set(String::new());
                let exts = parse_exts(&extensions.get_untracked());
                let mut body = serde_json::json!({ "extensions": exts, "claim_defaults": claim.get_untracked(), "confirm_conflicts": confirm_conflicts.get_untracked() });
                let id = id.clone();
                busy.set(true);
                wasm_bindgen_futures::spawn_local(async move {
                    let reply = api::send("PUT", &format!("/api/admin/apps/{id}/settings"), Some(&serde_json::to_vec(&body).unwrap_or_default()), Some("application/json"), None).await;
                    busy.set(false);
                    match reply {
                        Ok(r) if r.status == 409 => {
                            let msg = api::error_body(&r.body).map(|e| e.error).unwrap_or_else(|| t("This file type is already used.").to_string());
                            error.set(format!("{msg} {}", t("Tick “continue anyway” below if you still want to save.")));
                            confirm_conflicts.set(true);
                        }
                        Ok(r) if r.status >= 400 => error.set(api::error_body(&r.body).map(|e| e.error).unwrap_or_else(|| t("Could not save file types.").to_string())),
                        Ok(r) => {
                            if let Ok(view) = serde_json::from_slice::<AdminApps>(&r.body) {
                                on_close.run(());
                                on_done.run(Ok(view));
                            }
                        }
                        Err(err) => error.set(err),
                    }
                });
            }>
                <label class="stack">
                    <span>{t("File types")}</span>
                    <input class="field" prop:value=move || extensions.get() on:input=move |ev| extensions.set(event_target_value(&ev)) placeholder="md, txt" />
                    <span class="muted">{t("Comma-separated suffixes, for example md,txt.")}</span>
                </label>
                <label class="check"><input type="checkbox" prop:checked=move || claim.get() on:change=move |ev| claim.set(event_target_checked(&ev)) /><span>{t("Make this the default app for these types if an official app already opens them")}</span></label>
                <Show when=move || confirm_conflicts.get()>
                    <label class="check confirm-risk"><input type="checkbox" prop:checked=move || confirm_conflicts.get() on:change=move |ev| confirm_conflicts.set(event_target_checked(&ev)) /><span>{t("Continue anyway")}</span></label>
                </Show>
                <Show when=move || !error.get().is_empty()><p class="form-error" role="alert">{move || error.get()}</p></Show>
                <footer>
                    <button type="button" class="button" on:click=move |_| on_close.run(())>{t("Cancel")}</button>
                    <button type="submit" class="button primary" disabled=move || busy.get()>{t("Save")}</button>
                </footer>
            </form>
        </Dialog>
    }
}

#[component]
fn GrantsDialog(
    app: AdminApp,
    on_done: Callback<Result<AdminApps, String>>,
    on_close: Callback<()>,
) -> impl IntoView {
    let id = app.id.clone();
    let document = app.grants.iter().any(|g| g == "document") || !app.extensions.is_empty();
    let grant_drive = RwSignal::new(app.grants.iter().any(|g| g == "drive"));
    let grant_clipboard = RwSignal::new(app.grants.iter().any(|g| g == "clipboard"));
    let grant_fullscreen = RwSignal::new(app.grants.iter().any(|g| g == "fullscreen"));
    let error = RwSignal::new(String::new());
    let busy = RwSignal::new(false);
    view! {
        <Dialog title=tf("Grants for {app}", &[("app", &app.title)]) on_close=on_close>
            <form class="dialog-body" on:submit=move |ev| {
                ev.prevent_default();
                if busy.get_untracked() { return; }
                let mut grants: Vec<String> = Vec::new();
                if document { grants.push("document".into()); }
                if grant_drive.get_untracked() { grants.push("drive".into()); }
                if grant_clipboard.get_untracked() { grants.push("clipboard".into()); }
                if grant_fullscreen.get_untracked() { grants.push("fullscreen".into()); }
                let body = serde_json::json!({ "grants": grants });
                let id = id.clone();
                busy.set(true);
                wasm_bindgen_futures::spawn_local(async move {
                    match api::json::<AdminApps>("PUT", &format!("/api/admin/apps/{id}/settings"), &body).await {
                        Ok(view) => { on_close.run(()); on_done.run(Ok(view)); }
                        Err(err) => { busy.set(false); error.set(err); }
                    }
                });
            }>
                <p class="muted">{t("These limits apply on top of origin isolation. The app still cannot use the signed-in session. Mail and calendar access cannot be granted.")}</p>
                <div class="grant-list">
                    <label class="check"><input type="checkbox" prop:checked=document disabled=true /><span><strong>{t("Opened document")}</strong><span class="muted">{t("Edit and save the file that was opened, and create a new file through the shell's save dialog (you pick the folder and name).")}</span></span></label>
                    <label class="check"><input type="checkbox" prop:checked=move || grant_drive.get() on:change=move |ev| grant_drive.set(event_target_checked(&ev)) disabled=!document /><span><strong>{t("Other files on the drive")}</strong><span class="muted">{t("The app can open or insert other files you pick in the shell's file dialog. Only grant this if you trust the app with any file you might choose.")}</span></span></label>
                    <label class="check"><input type="checkbox" prop:checked=move || grant_clipboard.get() on:change=move |ev| grant_clipboard.set(event_target_checked(&ev)) /><span><strong>{t("Clipboard")}</strong><span class="muted">{t("Read from and write to the clipboard while the app is open.")}</span></span></label>
                    <label class="check"><input type="checkbox" prop:checked=move || grant_fullscreen.get() on:change=move |ev| grant_fullscreen.set(event_target_checked(&ev)) /><span><strong>{t("Full screen")}</strong><span class="muted">{t("Allow the app to enter full screen.")}</span></span></label>
                </div>
                <Show when=move || !error.get().is_empty()><p class="form-error" role="alert">{move || error.get()}</p></Show>
                <footer>
                    <button type="button" class="button" on:click=move |_| on_close.run(())>{t("Cancel")}</button>
                    <button type="submit" class="button primary" disabled=move || busy.get()>{t("Save")}</button>
                </footer>
            </form>
        </Dialog>
    }
}

#[component]
fn SourceDialog(
    app: AdminApp,
    on_done: Callback<Result<AdminApps, String>>,
    on_close: Callback<()>,
) -> impl IntoView {
    let id = app.id.clone();
    let existing = app.source.clone();
    let kind = RwSignal::new(
        existing
            .as_ref()
            .map(|s| s.kind.clone())
            .unwrap_or_else(|| "manifest".into()),
    );
    let url = RwSignal::new(existing.as_ref().map(|s| s.url.clone()).unwrap_or_default());
    let error = RwSignal::new(String::new());
    let busy = RwSignal::new(false);
    view! {
        <Dialog title=tf("Source for {app}", &[("app", &app.title)]) on_close=on_close>
            <form class="dialog-body" on:submit=move |ev| {
                ev.prevent_default();
                if busy.get_untracked() { return; }
                let source = AppSource { kind: kind.get_untracked(), url: url.get_untracked() };
                if source.url.trim().is_empty() {
                    error.set(t("Enter a GitHub owner/repo or an https manifest URL.").to_string());
                    return;
                }
                let id = id.clone();
                busy.set(true);
                wasm_bindgen_futures::spawn_local(async move {
                    match api::json::<AdminApps>("POST", &format!("/api/admin/apps/{id}/source"), &serde_json::to_value(&source).unwrap_or_default()).await {
                        Ok(view) => { on_close.run(()); on_done.run(Ok(view)); }
                        Err(err) => { busy.set(false); error.set(err); }
                    }
                });
            }>
                <label class="stack">
                    <span>{t("Source")}</span>
                    <select class="field" prop:value=move || kind.get() on:change=move |ev| kind.set(event_target_value(&ev))>
                        <option value="github">{t("GitHub releases")}</option>
                        <option value="manifest">{t("Manifest URL")}</option>
                    </select>
                </label>
                <label class="stack">
                    <span>{t("Source URL or owner/repo")}</span>
                    <input class="field" prop:value=move || url.get() on:input=move |ev| url.set(event_target_value(&ev)) placeholder="owner/repo or https://…" />
                    <span class="muted">{t("A GitHub owner/repo with the usual release zips, or an https URL to a manifest.")}</span>
                </label>
                <Show when=move || !error.get().is_empty()><p class="form-error" role="alert">{move || error.get()}</p></Show>
                <footer>
                    <button type="button" class="button" on:click=move |_| on_close.run(())>{t("Cancel")}</button>
                    {existing.is_some().then(|| {
                        let id = app.id.clone();
                        view! {
                            <button type="button" class="button danger" disabled=move || busy.get() on:click=move |_| {
                                let id = id.clone();
                                busy.set(true);
                                wasm_bindgen_futures::spawn_local(async move {
                                    match api::json::<AdminApps>("POST", &format!("/api/admin/apps/{id}/source"), &serde_json::Value::Null).await {
                                        Ok(view) => { on_close.run(()); on_done.run(Ok(view)); }
                                        Err(err) => { busy.set(false); error.set(err); }
                                    }
                                });
                            }>{t("Remove source")}</button>
                        }
                    })}
                    <button type="submit" class="button primary" disabled=move || busy.get()>{t("Save")}</button>
                </footer>
            </form>
        </Dialog>
    }
}

fn event_target<T: wasm_bindgen::JsCast>(ev: &web_sys::Event) -> Option<T> {
    ev.target().and_then(|t| t.dyn_into::<T>().ok())
}

fn isolation_label(mode: &str) -> String {
    match mode {
        "regular" => t("Regular (same origin)").to_string(),
        "domain" => t("Separate domain").to_string(),
        _ => t("Sandboxed iframe").to_string(),
    }
}

#[component]
fn IsolationPicker(
    mode: RwSignal<String>,
    confirm_regular: RwSignal<bool>,
    apps_origin: Option<String>,
    disabled: bool,
) -> impl IntoView {
    let domain_ok = apps_origin.is_some();
    let hint = apps_origin.clone().unwrap_or_default();
    view! {
        <fieldset class="grant-list isolation-picker" disabled=disabled>
            <legend>{t("Isolation")}</legend>
            <p class="muted">{t("Choose how this app is kept apart from the rest of artcraftCloud. You can change this later. Sandboxed iframe is the default.")}</p>
            <label class="check">
                <input type="radio" name="isolation" value="sandbox" prop:checked=move || mode.get() == "sandbox" on:change=move |_| mode.set("sandbox".into()) disabled=disabled />
                <span><strong>{t("Sandboxed iframe")}</strong><span class="muted">{t("Same site, opaque origin. No lasting localStorage, IndexedDB or cookies — settings go through the host bridge. Cannot use the signed-in session. No extra hostname.")}</span></span>
            </label>
            <label class="check confirm-risk">
                <input type="radio" name="isolation" value="regular" prop:checked=move || mode.get() == "regular" on:change=move |_| mode.set("regular".into()) disabled=disabled />
                <span><strong>{t("Regular (same origin)")}</strong><span class="muted">{t("Same origin as an official app: full browser storage (localStorage, IndexedDB, cookies). The app can act as the signed-in user. High risk — only for software you fully trust. Capability tokens are still issued for the opened file.")}</span></span>
            </label>
            <Show when=move || mode.get() == "regular">
                <p class="form-error" role="alert">{t("Danger: a regular sideloaded app can call the API with your session and keep data in the browser. Tick the extra confirmation below.")}</p>
                <label class="check confirm-risk"><input type="checkbox" prop:checked=move || confirm_regular.get() on:change=move |ev| confirm_regular.set(event_target_checked(&ev)) disabled=disabled /><span>{t("I understand this app can act as the signed-in user and I still want regular isolation.")}</span></label>
            </Show>
            <label class="check">
                <input type="radio" name="isolation" value="domain" prop:checked=move || mode.get() == "domain" on:change=move |_| mode.set("domain".into()) disabled=disabled || !domain_ok />
                <span><strong>{t("Separate domain")}</strong><span class="muted">{t("Served from ARTCRAFT_APPS_URL, with its own real origin and working browser storage. Cannot use the signed-in session. Needs a second hostname and certificate.")}</span></span>
            </label>
            <Show when=move || !domain_ok>
                <p class="muted">{t("Separate domain is unavailable until ARTCRAFT_APPS_URL is set to a second hostname for this same server (for example https://apps.your-domain). In Helm, set appsUrl and ingress.appsHost, then reload this page.")}</p>
            </Show>
            <Show when=move || domain_ok>
                <p class="muted">{tf("Apps origin is {origin}.", &[("origin", &hint)])}</p>
            </Show>
            <table class="isolation-compare">
                <caption class="muted">{t("How the modes compare")}</caption>
                <thead>
                    <tr>
                        <th></th>
                        <th>{t("Sandboxed iframe")}</th>
                        <th>{t("Regular (same origin)")}</th>
                        <th>{t("Separate domain")}</th>
                    </tr>
                </thead>
                <tbody>
                    <tr>
                        <th>{t("Browser storage")}</th>
                        <td>{t("None — settings go through the bridge")}</td>
                        <td>{t("Full (localStorage, IndexedDB, cookies)")}</td>
                        <td>{t("Own origin (localStorage, IndexedDB, cookies on the apps host)")}</td>
                    </tr>
                    <tr>
                        <th>{t("Signed-in session")}</th>
                        <td>{t("No")}</td>
                        <td>{t("Yes — the app can act as the signed-in user")}</td>
                        <td>{t("No")}</td>
                    </tr>
                    <tr>
                        <th>{t("Setup")}</th>
                        <td>{t("None (default)")}</td>
                        <td>{t("None, plus the extra confirmation")}</td>
                        <td>{t("ARTCRAFT_APPS_URL and a second hostname")}</td>
                    </tr>
                    <tr>
                        <th>{t("Risk")}</th>
                        <td>{t("Low — opaque origin")}</td>
                        <td>{t("High — treat this like giving the app your login")}</td>
                        <td>{t("Low — no session, own origin")}</td>
                    </tr>
                </tbody>
            </table>
        </fieldset>
    }
}

#[component]
fn IsolationDialog(
    app: AdminApp,
    apps_origin: Option<String>,
    on_done: Callback<Result<AdminApps, String>>,
    on_close: Callback<()>,
) -> impl IntoView {
    let id = app.id.clone();
    let current = if app.isolation.is_empty() {
        "sandbox".into()
    } else {
        app.isolation.clone()
    };
    let mode = RwSignal::new(current);
    let confirm_regular = RwSignal::new(false);
    let error = RwSignal::new(String::new());
    let busy = RwSignal::new(false);
    view! {
        <Dialog title=tf("Isolation for {app}", &[("app", &app.title)]) on_close=on_close>
            <form class="dialog-body" on:submit=move |ev| {
                ev.prevent_default();
                if busy.get_untracked() { return; }
                let isolation = mode.get_untracked();
                let body = serde_json::json!({ "isolation": isolation, "confirm_regular": confirm_regular.get_untracked() });
                let id = id.clone();
                busy.set(true);
                wasm_bindgen_futures::spawn_local(async move {
                    match api::json::<AdminApps>("PUT", &format!("/api/admin/apps/{id}/settings"), &body).await {
                        Ok(view) => { on_close.run(()); on_done.run(Ok(view)); }
                        Err(err) => { busy.set(false); error.set(err); }
                    }
                });
            }>
                <IsolationPicker mode=mode confirm_regular=confirm_regular apps_origin=apps_origin.clone() disabled=false />
                <Show when=move || !error.get().is_empty()><p class="form-error" role="alert">{move || error.get()}</p></Show>
                <footer>
                    <button type="button" class="button" on:click=move |_| on_close.run(())>{t("Cancel")}</button>
                    <button type="submit" class="button primary" disabled=move || busy.get()>{t("Save")}</button>
                </footer>
            </form>
        </Dialog>
    }
}
