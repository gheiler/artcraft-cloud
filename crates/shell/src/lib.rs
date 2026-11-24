mod api;
mod apps_admin;
mod catalog;
mod drive;
mod editor;
mod i18n;
mod icons;
mod mail;
mod modal;
mod people;
mod picker;
mod preferences;
mod route;
mod settings;
mod ui;

use crate::i18n::t;
use catalog::app_by_id;
use drive::{open_file, Drive};
use editor::{install, EditorFrame, Launch, SaveKind};
use icons::{icon, Icon};
use leptos::prelude::*;
use modal::Modals;
use people::People;
use route::Route;
use ui::{ui, Modal, Ui};
use wasm_bindgen::{closure::Closure, JsCast};

fn location_route() -> Route {
    let Some(window) = web_sys::window() else {
        return Route::Files(String::new());
    };
    let location = window.location();
    Route::parse(
        &location.pathname().unwrap_or_default(),
        &location.search().unwrap_or_default(),
    )
}

#[component]
pub fn App() -> impl IntoView {
    let ui = Ui::new();
    ui::set_global(ui);
    provide_context(ui);
    let editor = ui.editor;
    let checked = RwSignal::new(false);
    install(editor);
    settings::install();
    install_history();
    install_escape();
    ui.route.set(location_route());

    wasm_bindgen_futures::spawn_local(async move {
        if let Ok(me) = api::me().await {
            editor.author.set(me.display_name.clone());
            let prefs = api::preferences().await;
            let super_admin = me.super_admin;
            ui.user.set(Some(me));
            if super_admin {
                apps_admin::load_alerts(ui);
            }
            if let Ok(prefs) = prefs {
                preferences::adopt(prefs);
            }
            editor.load_shell_settings();
            if let Ok(list) = api::apps().await {
                catalog::adopt(ui, list);
            }
        } else if let Ok(list) = api::catalog().await {
            catalog::adopt(ui, list);
        }
        checked.set(true);
    });

    // Documents follow the address: opening a file, a reload or a pasted link all land here.
    Effect::new(move |_| {
        let route = ui.route.get();
        let signed_in = ui.user.with(Option::is_some);
        if !checked.get() || !signed_in {
            return;
        }
        match route {
            Route::Open { share, path, app } => {
                let kind = share.map(SaveKind::Share).unwrap_or(SaveKind::Home);
                let showing = editor.open.get_untracked()
                    && editor.kind.get_untracked() == kind
                    && (editor.path.get_untracked().as_deref() == Some(path.as_str())
                        || editor.suggested.get_untracked().as_deref() == Some(path.as_str()))
                    && app
                        .as_ref()
                        .is_none_or(|app| *app == editor.app.get_untracked());
                if !showing {
                    let name = editor::file_name(&path);
                    open_file(kind, path, name, app);
                }
            }
            Route::New { app, folder } => {
                let fresh = editor.open.get_untracked()
                    && editor.path.get_untracked().is_none()
                    && editor.app.get_untracked() == app;
                if !fresh {
                    let item = app_by_id(&app);
                    let ext = item
                        .and_then(|a| a.extensions.first().copied())
                        .unwrap_or("txt");
                    // An app without the host bridge opens no document: its title is the name.
                    let name = match item {
                        Some(item) if !item.bridge => item.title.to_string(),
                        _ => format!("Untitled.{ext}"),
                    };
                    editor.launch(Launch {
                        app: app.clone(),
                        name,
                        folder,
                        kind: Some(SaveKind::Home),
                        ..Launch::default()
                    });
                }
            }
            Route::Public(_) => {}
            _ => {
                if editor.open.get_untracked() {
                    editor.close_now();
                }
            }
        }
    });

    view! {
        <div class="shell" class:collapsed=move || ui.sidebar_collapsed.get() class:doc-open=move || ui.editor.open.get()>
            <Show when=move || checked.get() fallback=|| view! { <div class="boot"><span class="spinner"></span></div> }>
                <Show when=move || ui.user.get().is_none() && !matches!(ui.route.get(), Route::Public(_)) fallback=move || view! { <Workspace /> }>
                    <Login />
                </Show>
            </Show>
            <Modals />
            <Show when=move || !ui.toast.get().is_empty()>
                <div class="toast" role="status">
                    <span>{move || ui.toast.get()}</span>
                    <button type="button" class="icon-button" aria-label=t("Dismiss") on:click=move |_| ui.toast.set(String::new())>{icon(Icon::Close)}</button>
                </div>
            </Show>
        </div>
    }
}

/// Back and Forward. Leaving a document with unsaved changes asks first.
fn install_history() {
    let Some(window) = web_sys::window() else {
        return;
    };
    let on_pop = Closure::<dyn FnMut()>::new(move || {
        let ui = ui();
        let next = location_route();
        let current = ui.route.get_untracked();
        if ui.editor.open.get_untracked()
            && ui.editor.dirty.get_untracked()
            && current.is_doc()
            && !next.is_doc()
        {
            // Stay on the document until the user decides.
            if let Some(history) = web_sys::window().and_then(|w| w.history().ok()) {
                let _ = history.push_state_with_url(
                    &wasm_bindgen::JsValue::NULL,
                    "",
                    Some(&current.href()),
                );
            }
            ui.open_modal(Modal::Discard { then: Some(next) });
            return;
        }
        if !next.is_doc() {
            ui.from_drive.set(false);
        }
        ui.close_modal();
        ui.route.set(next);
    });
    let _ = window.add_event_listener_with_callback("popstate", on_pop.as_ref().unchecked_ref());
    on_pop.forget();
}

/// Escape closes the open dialog wherever the focus is.
fn install_escape() {
    let Some(window) = web_sys::window() else {
        return;
    };
    let on_key =
        Closure::<dyn FnMut(web_sys::KeyboardEvent)>::new(move |event: web_sys::KeyboardEvent| {
            if event.key() != "Escape" {
                return;
            }
            let ui = ui();
            match ui.modal.get_untracked() {
                Some(Modal::Pick { id, .. }) => {
                    picker::dismiss(id);
                    ui.close_modal();
                }
                Some(_) => ui.close_modal(),
                None => {}
            }
        });
    let _ = window.add_event_listener_with_callback("keydown", on_key.as_ref().unchecked_ref());
    on_key.forget();
}

#[component]
fn Login() -> impl IntoView {
    let ui = ui();
    let username = RwSignal::new(String::new());
    let password = RwSignal::new(String::new());
    let error = RwSignal::new(String::new());
    view! {
        <main class="login">
            <form class="card login-card" on:submit=move |ev| {
                ev.prevent_default();
                let username = username.get_untracked();
                let password = password.get_untracked();
                wasm_bindgen_futures::spawn_local(async move {
                    match api::login(&username, &password).await {
                        Ok(me) => {
                            ui.editor.author.set(me.display_name.clone());
                            let _ = error.try_set(String::new());
                            let prefs = api::preferences().await;
                            ui.user.set(Some(me));
                            if let Ok(prefs) = prefs {
                                preferences::adopt(prefs);
                            }
                            ui.editor.load_shell_settings();
                            if let Ok(list) = api::apps().await {
                                catalog::adopt(ui, list);
                            }
                            // Re-read the address: a document link opens once signed in.
                            ui.route.set(location_route());
                        }
                        Err(err) => {
                            let _ = error.try_set(err);
                        }
                    }
                });
            }>
                <div class="brand big"><span class="logo-mark"></span><span>"artcraft"<b>"Cloud"</b></span></div>
                <p class="muted">{t("Your files, in the Crafting Apps.")}</p>
                <label class="stack"><span>{t("Username")}</span><input class="field" prop:value=move || username.get() on:input=move |ev| username.set(event_target_value(&ev)) autocomplete="username" /></label>
                <label class="stack"><span>{t("Password")}</span><input class="field" type="password" prop:value=move || password.get() on:input=move |ev| password.set(event_target_value(&ev)) autocomplete="current-password" /></label>
                <button type="submit" class="button primary block">{t("Sign in")}</button>
                <p class="error">{move || error.get()}</p>
            </form>
        </main>
    }
}

#[component]
fn Workspace() -> impl IntoView {
    let ui = ui();
    let nav = move |label: &'static str, glyph: Icon, target: Route| {
        let active = target.clone();
        view! {
            <button type="button" class="nav" title=label
                class:active=move || {
                    let route = ui.route.get();
                    match (&active, &route) {
                        (Route::Files(_), Route::Files(_)) => true,
                        (Route::Shared, Route::Shared | Route::Share(_, _)) => true,
                        (a, b) => a == b,
                    }
                }
                on:click=move |_| ui.go(target.clone())>
                {icon(glyph)}<span class="nav-label">{label}</span>
            </button>
        }
    };
    let signed_in = move || ui.user.get().is_some();
    view! {
        <Show when=signed_in>
            <aside class="sidebar" aria-label=t("Navigation")>
                <div class="sidebar-head">
                    <div class="brand"><span class="logo-mark"></span><span class="nav-label">"artcraft"<b>"Cloud"</b></span></div>
                    <button type="button" class="icon-button" title=move || if ui.sidebar_collapsed.get() { t("Expand sidebar") } else { t("Collapse sidebar") }
                        aria-label=t("Toggle sidebar") on:click=move |_| ui.toggle_sidebar()>{icon(Icon::Sidebar)}</button>
                </div>
                <nav>
                    {nav(t("My files"), Icon::Folder, Route::Files(String::new()))}
                    {nav(t("Shared"), Icon::Shared, Route::Shared)}
                    {nav(t("Trash"), Icon::Trash, Route::Trash)}
                    {nav(t("Settings"), Icon::Settings, Route::Settings)}
                    <Show when=move || ui.is_admin()>
                        {nav(t("People"), Icon::People, Route::People)}
                    </Show>
                    <Show when=move || ui.user.with(|u| u.as_ref().is_some_and(|u| u.super_admin))>
                        {nav(t("Mail"), Icon::Mail, Route::Mail)}
                        {nav(t("Apps"), Icon::Apps, Route::Apps)}
                    </Show>
                </nav>
                <div class="sidebar-who">
                    <span class="avatar">{move || modal::initials(&ui.user.get().map(|u| u.display_name).unwrap_or_default())}</span>
                    <span class="nav-label who-name">
                        <strong>{move || ui.user.get().map(|u| u.display_name).unwrap_or_default()}</strong>
                        <span class="muted">{move || ui.user.get().map(|u| if u.super_admin { t("Super admin") } else if u.role == "admin" { t("Admin") } else { t("Member") }).unwrap_or_default()}</span>
                    </span>
                    <button type="button" class="icon-button" title=t("Sign out") aria-label=t("Sign out") on:click=move |_| {
                        wasm_bindgen_futures::spawn_local(async move {
                            let _ = api::logout().await;
                            ui.editor.close_now();
                            settings::forget_local();
                            ui.user.set(None);
                            ui.replace(Route::Files(String::new()));
                        });
                    }>{icon(Icon::Logout)}</button>
                </div>
            </aside>
        </Show>
        <main class="content">
            <Show when=move || !ui.route.get().is_doc()>
                <apps_admin::AppAlerts />
                <Show when=move || matches!(ui.route.get(), Route::Settings) fallback=move || view! {
                    <Show when=move || matches!(ui.route.get(), Route::People) fallback=move || view! {
                        <Show when=move || matches!(ui.route.get(), Route::Mail) fallback=move || view! {
                            <Show when=move || matches!(ui.route.get(), Route::Apps) fallback=|| view! { <Drive /> }>
                                <Show when=move || ui.user.with(|u| u.as_ref().is_some_and(|u| u.super_admin)) fallback=|| view! { <p class="empty">{t("Only the super admin manages apps.")}</p> }>
                                    <apps_admin::AppsPage />
                                </Show>
                            </Show>
                        }>
                            <Show when=move || ui.user.with(|u| u.as_ref().is_some_and(|u| u.super_admin)) fallback=|| view! { <p class="empty">{t("Only the super admin sets up mail.")}</p> }>
                                <mail::MailPage />
                            </Show>
                        </Show>
                    }>
                        <Show when=move || ui.is_admin() fallback=|| view! { <p class="empty">{t("Only admins manage people.")}</p> }>
                            <People />
                        </Show>
                    </Show>
                }>
                    <preferences::Settings />
                </Show>
            </Show>
            <EditorFrame />
        </main>
    }
}

#[cfg(target_arch = "wasm32")]
#[wasm_bindgen::prelude::wasm_bindgen(start)]
pub fn start() {
    console_error_panic_hook::set_once();
    preferences::boot();
    leptos::mount::mount_to_body(App);
}
