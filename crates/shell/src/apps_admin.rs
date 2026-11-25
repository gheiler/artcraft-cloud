//! Apps (super admin): what the signed app registry offers, what is installed, and adding,
//! updating, rolling back or removing an app while the server runs. The server does the work and
//! checks every version against the registry; this page shows it and polls running installs.

use artcraft_cloud_types::{AdminApp, AdminApps, AppAlert, AppJob};
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
                        <span class="badge" class:danger=state == "blocked" || state == "incompatible">{state_label(&state)}</span>
                        {app.reason.clone().map(|r| view! { <span class="muted">{r}</span> })}
                        {disabled.then(|| view! { <span class="badge danger">{t("Disabled")}</span> })}
                        {app.update.clone().map(|v| view! { <span class="badge primary">{tf("Update available: {version}", &[("version", &v)])}</span> })}
                        {job.map(|j| view! { <span class="badge">{phase_label(&j)}</span> })}
                    </span>
                </div>
                <div class="app-row-actions">
                    <label class="check" title=t("Install new allowed versions on its own (checked every 6 hours)")>
                        <input type="checkbox" prop:checked=app.auto_update disabled=!app.in_registry on:change=move |ev| settings(a1.id.clone(), serde_json::json!({ "auto_update": event_target_checked(&ev) })) />
                        <span>{t("Auto-update")}</span>
                    </label>
                    {app.update.clone().map(|v| { let id = id.clone(); view! { <button type="button" class="button primary small" on:click=move |_| install(id.clone(), Some(v.clone()))>{t("Update")}</button> } })}
                    {app.in_registry.then(|| view! { <button type="button" class="button small" on:click=move |_| versions_for.set(Some(a2.id.clone()))>{t("Versions…")}</button> })}
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
                    <p class="muted">{t("Only versions the app registry allows can be installed. Each is checked against the registry's checksum and the publisher's signature.")}</p>
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
            </Show>
            {job_dialog}
            {versions_dialog}
        </section>
    }
}
