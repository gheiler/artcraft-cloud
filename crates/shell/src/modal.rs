//! Dialogs. Every question the shell asks goes through one of these instead of the browser's
//! `alert`, `prompt` or `confirm`.

use crate::i18n::{t, tf};
use artcraft_cloud_types::{app_by_id, Directory, FileEntry, Share};
use leptos::prelude::*;

use crate::api;
use crate::drive::{app_of_name, apps_for, share_target};
use crate::editor::{file_name, finish_leave, parent_folder};
use crate::icons::{app_icon, icon, Icon};
use crate::picker;
use crate::route::Route;
use crate::ui::{ui, Modal};

#[component]
pub fn Modals() -> impl IntoView {
    let ui = ui();
    move || {
        ui.modal.get().map(|modal| match modal {
            Modal::NewFolder { dir } => view! { <NameDialog title=t("New folder").into() label=t("Folder name").into() initial=String::new() confirm=t("Create").into() on_ok=Callback::new(move |name: String| {
                let path = if dir.is_empty() { name } else { format!("{dir}/{name}") };
                run(api::req("POST", "/api/files/mkdir", Some(serde_json::json!({"path": path}))), None);
            }) /> }.into_any(),
            Modal::Rename { path } => {
                let current = file_name(&path);
                view! { <NameDialog title=tf("Rename {current}", &[("current", &current)]) label=t("New name").into() initial=current confirm=t("Rename").into() on_ok=Callback::new(move |name: String| {
                    run(api::req("POST", "/api/files/rename", Some(serde_json::json!({"path": path, "name": name}))), None);
                }) /> }.into_any()
            }
            Modal::Transfer { path, copy } => view! { <FolderDialog path=path copy=copy /> }.into_any(),
            Modal::Trash { path } => {
                let name = file_name(&path);
                view! { <ConfirmDialog title=tf("Move {name} to the trash?", &[("name", &name)]) body=t("You can restore it from the trash later.").into() confirm=t("Move to trash").into() danger=true
                    on_ok=Callback::new(move |_| run(api::req("POST", "/api/files/delete", Some(serde_json::json!({"path": path}))), Some(t("Moved to the trash.")))) /> }.into_any()
            }
            Modal::Purge { id: Some(id) } => view! { <ConfirmDialog title=t("Delete forever?").into() body=t("This cannot be undone.").into() confirm=t("Delete forever").into() danger=true
                on_ok=Callback::new(move |_| run(api::req("DELETE", format!("/api/trash/{id}"), None), None)) /> }.into_any(),
            Modal::Purge { id: None } => view! { <ConfirmDialog title=t("Empty the trash?").into() body=t("Everything in the trash is deleted for good. This cannot be undone.").into() confirm=t("Empty trash").into() danger=true
                on_ok=Callback::new(move |_| run(api::req("DELETE", "/api/trash", None), None)) /> }.into_any(),
            Modal::Discard { then } => view! { <ConfirmDialog title=t("Discard unsaved changes?").into() body=t("Your latest changes to this document have not been saved to the drive.").into() confirm=t("Discard changes").into() danger=true
                on_ok=Callback::new(move |_| finish_leave(then.clone())) /> }.into_any(),
            Modal::Share { path, is_dir } => view! { <ShareDialog path=path is_dir=is_dir /> }.into_any(),
            Modal::OpenWith { path, share } => view! { <OpenWithDialog path=path share=share /> }.into_any(),
            Modal::Pick { id, accept, multiple } => view! { <PickDialog id=id accept=accept multiple=multiple /> }.into_any(),
            Modal::ResetPassword { id, username } => view! { <NameDialog title=tf("New password for {username}", &[("username", &username)]) label=t("Password (at least 10 characters)").into() initial=String::new() confirm=t("Set password").into() password=true
                on_ok=Callback::new(move |password: String| run(api::req("POST", format!("/api/users/{id}/password"), Some(serde_json::json!({"password": password}))), Some(t("Password changed. Their sessions were signed out.")))) /> }.into_any(),
        })
    }
}

/// Run a request, then close the dialog and reload the page's data (or show the error).
fn run(
    request: impl std::future::Future<Output = Result<(), String>> + 'static,
    done: Option<&'static str>,
) {
    let ui = ui();
    wasm_bindgen_futures::spawn_local(async move {
        match request.await {
            Ok(()) => {
                if let Some(message) = done {
                    ui.say(message);
                }
            }
            Err(err) => ui.say(err),
        }
        ui.refresh();
    });
}

/// The frame every dialog shares: a dimmed page, a titled card, Escape or the × to close.
#[component]
fn Dialog(
    title: String,
    #[prop(optional)] wide: bool,
    #[prop(optional)] on_close: Option<Callback<()>>,
    children: Children,
) -> impl IntoView {
    let ui = ui();
    let close = move || {
        if let Some(callback) = on_close {
            callback.run(());
        }
        ui.close_modal();
    };
    view! {
        <div class="overlay" on:mousedown=move |ev| {
            if ev.target() == ev.current_target() {
                close();
            }
        }>
            <div class="dialog" class:wide=wide role="dialog" aria-modal="true" aria-label=title.clone()>
                <header>
                    <h2>{title.clone()}</h2>
                    <button type="button" class="icon-button" aria-label=t("Close") on:click=move |_| close()>{icon(Icon::Close)}</button>
                </header>
                {children()}
            </div>
        </div>
    }
}

fn focus_on_load(node: NodeRef<leptos::html::Input>, select_stem: bool) {
    Effect::new(move |_| {
        let Some(input) = node.get() else { return };
        let input: web_sys::HtmlInputElement = input;
        let _ = input.focus();
        if select_stem {
            let value = input.value();
            let end = value.rfind('.').filter(|i| *i > 0).unwrap_or(value.len());
            let _ = input.set_selection_range(0, end as u32);
        }
    });
}

#[component]
fn NameDialog(
    title: String,
    label: String,
    initial: String,
    confirm: String,
    on_ok: Callback<String>,
    #[prop(optional)] password: bool,
) -> impl IntoView {
    let ui = ui();
    let value = RwSignal::new(initial);
    let input = NodeRef::<leptos::html::Input>::new();
    focus_on_load(input, !password);
    view! {
        <Dialog title=title>
            <form class="dialog-body" on:submit=move |ev| {
                ev.prevent_default();
                let text = value.get_untracked().trim().to_string();
                if text.is_empty() {
                    return;
                }
                ui.close_modal();
                on_ok.run(text);
            }>
                <label class="stack">
                    <span>{label}</span>
                    <input class="field" node_ref=input type=if password { "password" } else { "text" } autocomplete="off"
                        prop:value=move || value.get() on:input=move |ev| value.set(event_target_value(&ev)) />
                </label>
                <footer>
                    <button type="button" class="button" on:click=move |_| ui.close_modal()>{t("Cancel")}</button>
                    <button type="submit" class="button primary" disabled=move || value.get().trim().is_empty()>{confirm}</button>
                </footer>
            </form>
        </Dialog>
    }
}

#[component]
fn ConfirmDialog(
    title: String,
    body: String,
    confirm: String,
    danger: bool,
    on_ok: Callback<()>,
) -> impl IntoView {
    let ui = ui();
    let button = NodeRef::<leptos::html::Button>::new();
    Effect::new(move |_| {
        if let Some(b) = button.get() {
            let b: web_sys::HtmlButtonElement = b;
            let _ = b.focus();
        }
    });
    view! {
        <Dialog title=title>
            <div class="dialog-body">
                <p>{body}</p>
                <footer>
                    <button type="button" class="button" on:click=move |_| ui.close_modal()>{t("Cancel")}</button>
                    <button type="button" node_ref=button class="button" class:danger-fill=danger class:primary=!danger on:click=move |_| {
                        ui.close_modal();
                        on_ok.run(());
                    }>{confirm}</button>
                </footer>
            </div>
        </Dialog>
    }
}

/// Browse folders of My files and pick one (move, copy).
#[component]
fn FolderDialog(path: String, copy: bool) -> impl IntoView {
    let ui = ui();
    let start = parent_folder(&path);
    let folder = RwSignal::new(start.clone());
    let dirs = RwSignal::new(Vec::<FileEntry>::new());
    Effect::new(move |_| {
        let at = folder.get();
        wasm_bindgen_futures::spawn_local(async move {
            match api::list_dir(&at).await {
                Ok(listing) => {
                    let mut list: Vec<FileEntry> =
                        listing.entries.into_iter().filter(|e| e.is_dir).collect();
                    list.sort_by_key(|e| e.name.to_lowercase());
                    let _ = dirs.try_set(list);
                }
                Err(err) => ui.say(err),
            }
        });
    });
    let name = file_name(&path);
    let moving = path.clone();
    let title = if copy { tf("Copy {name}", &[("name", &name)]) } else { tf("Move {name}", &[("name", &name)]) };
    let blocked = move || {
        let at = folder.get();
        (!copy && at == start) || at == moving || at.starts_with(&format!("{moving}/"))
    };
    let source = path.clone();
    view! {
        <Dialog title=title wide=false>
            <div class="dialog-body">
                <div class="picker-path">
                    <button type="button" class="crumb" on:click=move |_| folder.set(String::new())>{icon(Icon::Home)}<span>{t("My files")}</span></button>
                    {move || {
                        let mut acc = String::new();
                        folder.get().split('/').filter(|s| !s.is_empty()).map(|segment| {
                            acc = if acc.is_empty() { segment.to_string() } else { format!("{acc}/{segment}") };
                            let target = acc.clone();
                            view! { <span class="sep">{icon(Icon::Chevron)}</span><button type="button" class="crumb" on:click=move |_| folder.set(target.clone())>{segment.to_string()}</button> }
                        }).collect_view()
                    }}
                </div>
                <ul class="picker-list">
                    <For each=move || dirs.get() key=|e| e.path.clone() let(entry)>
                        <li><button type="button" class="file-name" disabled=entry.path == path on:click={
                            let target = entry.path.clone();
                            move |_| folder.set(target.clone())
                        }><span class="file-icon">{icon(Icon::Folder)}</span><span class="label">{entry.name.clone()}</span></button></li>
                    </For>
                    <Show when=move || dirs.get().is_empty()><li class="empty">{t("No folders inside.")}</li></Show>
                </ul>
                <footer>
                    <button type="button" class="button" on:click=move |_| ui.close_modal()>{t("Cancel")}</button>
                    <button type="button" class="button primary" disabled=blocked on:click=move |_| {
                        let to_dir = folder.get_untracked();
                        let url = if copy { "/api/files/copy" } else { "/api/files/move" };
                        let body = serde_json::json!({"from": source, "to_dir": to_dir});
                        ui.close_modal();
                        run(async move { api::call("POST", url, Some(&body)).await }, Some(if copy { t("Copied.") } else { t("Moved.") }));
                    }>{if copy { t("Copy here") } else { t("Move here") }}</button>
                </footer>
            </div>
        </Dialog>
    }
}

#[derive(Clone, PartialEq)]
enum Target {
    User(String, String),
    Group(String, usize),
}

impl Target {
    fn key(&self) -> String {
        match self {
            Target::User(name, _) => format!("u:{name}"),
            Target::Group(name, _) => format!("g:{name}"),
        }
    }
}

/// Share with people and groups, and make links.
#[component]
fn ShareDialog(path: String, is_dir: bool) -> impl IntoView {
    let ui = ui();
    let directory = RwSignal::new(Directory::default());
    let existing = RwSignal::new(Vec::<Share>::new());
    let query = RwSignal::new(String::new());
    let picked = RwSignal::new(Vec::<Target>::new());
    let permission = RwSignal::new(String::from("view"));
    let link_permission = RwSignal::new(String::from("view"));
    let link_password = RwSignal::new(String::new());
    let link_expiry = RwSignal::new(String::new());
    let reload = RwSignal::new(0u32);
    let working = RwSignal::new(false);
    let path_for_load = path.clone();
    wasm_bindgen_futures::spawn_local(async move {
        if let Ok(dir) = api::directory().await {
            let _ = directory.try_set(dir);
        }
    });
    Effect::new(move |_| {
        reload.get();
        let path = path_for_load.clone();
        wasm_bindgen_futures::spawn_local(async move {
            if let Ok(list) = api::shares(false).await {
                let _ = existing.try_set(list.into_iter().filter(|s| s.path == path).collect());
            }
        });
    });
    let suggestions = move || {
        let q = query.get().trim().to_lowercase();
        let chosen: Vec<String> = picked.get().iter().map(Target::key).collect();
        let shared: Vec<String> = existing
            .get()
            .iter()
            .filter(|s| !s.public)
            .map(|s| match &s.group {
                Some(g) => format!("g:{g}"),
                None => format!("u:{}", s.grantee.clone().unwrap_or_default()),
            })
            .collect();
        let dir = directory.get();
        let mut out: Vec<Target> = dir
            .groups
            .iter()
            .map(|g| Target::Group(g.name.clone(), g.members))
            .chain(
                dir.users
                    .iter()
                    .map(|u| Target::User(u.username.clone(), u.display_name.clone())),
            )
            .filter(|t| !chosen.contains(&t.key()) && !shared.contains(&t.key()))
            .filter(|t| {
                q.is_empty()
                    || match t {
                        Target::User(name, display) => {
                            name.to_lowercase().contains(&q) || display.to_lowercase().contains(&q)
                        }
                        Target::Group(name, _) => name.to_lowercase().contains(&q),
                    }
            })
            .collect();
        out.truncate(8);
        out
    };
    let path_share = path.clone();
    let share_now = move |_| {
        let targets = picked.get_untracked();
        if targets.is_empty() {
            return;
        }
        let permission = permission.get_untracked();
        let path = path_share.clone();
        working.set(true);
        wasm_bindgen_futures::spawn_local(async move {
            for target in targets {
                let body = match &target {
                    Target::User(name, _) => {
                        serde_json::json!({"path": path, "grantee": name, "permission": permission, "public": false})
                    }
                    Target::Group(name, _) => {
                        serde_json::json!({"path": path, "group": name, "permission": permission, "public": false})
                    }
                };
                if let Err(err) = api::post_json::<_, Share>("/api/shares", &body).await {
                    ui.say(err);
                }
            }
            let _ = picked.try_set(Vec::new());
            let _ = query.try_set(String::new());
            let _ = working.try_set(false);
            let _ = reload.try_update(|n| *n += 1);
            ui.refresh();
        });
    };
    let path_link = path.clone();
    let make_link = move |_| {
        let expires_at = parse_date(&link_expiry.get_untracked());
        let password = link_password.get_untracked();
        let body = serde_json::json!({
            "path": path_link,
            "permission": link_permission.get_untracked(),
            "public": true,
            "password": if password.is_empty() { serde_json::Value::Null } else { serde_json::Value::String(password) },
            "expires_at": expires_at,
        });
        wasm_bindgen_futures::spawn_local(async move {
            match api::post_json::<_, Share>("/api/shares", &body).await {
                Ok(share) => {
                    if let Some(token) = share.token {
                        copy_text(&link_of(&token));
                        ui.say(t("Link created and copied."));
                    }
                }
                Err(err) => ui.say(err),
            }
            let _ = link_password.try_set(String::new());
            let _ = reload.try_update(|n| *n += 1);
        });
    };
    let path_change = path.clone();
    let name = file_name(&path);
    let people_rows = move || {
        existing
            .get()
            .into_iter()
            .filter(|s| !s.public)
            .collect::<Vec<_>>()
    };
    let link_rows = move || {
        existing
            .get()
            .into_iter()
            .filter(|s| s.public)
            .collect::<Vec<_>>()
    };
    view! {
        <Dialog title=tf("Share {name}", &[("name", &name)]) wide=true>
            <div class="dialog-body share">
                <p class="muted">{if is_dir { t("People you add see everything in this folder.") } else { t("People you add find it under Shared.") }}</p>
                <div class="picker">
                    <div class="token-field">
                        {move || picked.get().into_iter().map(|target| {
                            let key = target.key();
                            let label = match &target { Target::User(_, display) => display.clone(), Target::Group(name, _) => name.clone() };
                            let is_group = matches!(target, Target::Group(..));
                            view! {
                                <span class="token">
                                    {if is_group { icon(Icon::People) } else { icon(Icon::Shared) }}
                                    {label}
                                    <button type="button" aria-label=t("Remove") on:click=move |_| picked.update(|list| list.retain(|t| t.key() != key))>{icon(Icon::Close)}</button>
                                </span>
                            }
                        }).collect_view()}
                        <span class="muted search-glyph">{icon(Icon::Search)}</span>
                        <input class="bare" placeholder=t("Add people or groups") prop:value=move || query.get() on:input=move |ev| query.set(event_target_value(&ev)) />
                    </div>
                    <select class="field narrow" aria-label=t("Permission") on:change=move |ev| permission.set(event_target_value(&ev)) prop:value=move || permission.get()>
                        <option value="view">{t("Can view")}</option>
                        <option value="edit">{t("Can edit")}</option>
                    </select>
                    <button type="button" class="button primary" disabled=move || picked.get().is_empty() || working.get() on:click=share_now>{t("Share")}</button>
                </div>
                <ul class="suggestions">
                    {move || suggestions().into_iter().map(|target| {
                        let add = target.clone();
                        let (title, detail, is_group) = match &target {
                            Target::User(name, display) => (display.clone(), name.clone(), false),
                            Target::Group(name, members) => (name.clone(), if *members == 1 { t("Group · 1 member").to_string() } else { tf("Group · {count} members", &[("count", &members.to_string())]) }, true),
                        };
                        view! {
                            <li><button type="button" class="suggestion" on:click=move |_| {
                                picked.update(|list| list.push(add.clone()));
                                query.set(String::new());
                            }>
                                <span class="avatar" class:group=is_group>{if is_group { icon(Icon::People).into_any() } else { initials(&title).into_any() }}</span>
                                <span class="who"><strong>{title}</strong><span class="muted">{detail}</span></span>
                                <span class="add">{icon(Icon::Plus)}</span>
                            </button></li>
                        }
                    }).collect_view()}
                </ul>
                <h3>{t("People with access")}</h3>
                <ul class="access">
                    <Show when=move || existing.get().iter().all(|s| s.public)><li class="empty">{t("Only you.")}</li></Show>
                    <For each=people_rows key=|s| (s.id, s.permission.clone()) let(share)>
                        <li>
                            <span class="avatar" class:group=share.group.is_some()>{if share.group.is_some() { icon(Icon::People).into_any() } else { initials(&share.grantee.clone().unwrap_or_default()).into_any() }}</span>
                            <span class="who"><strong>{share_target(&share)}</strong></span>
                            <select class="field narrow" aria-label=t("Permission") prop:value=share.permission.clone() on:change={
                                let path = path_change.clone();
                                let share = share.clone();
                                move |ev| {
                                    let body = serde_json::json!({
                                        "path": path, "permission": event_target_value(&ev), "public": false,
                                        "grantee": share.group.is_none().then(|| share.grantee.clone()).flatten(),
                                        "group": share.group.clone(),
                                    });
                                    wasm_bindgen_futures::spawn_local(async move {
                                        if let Err(err) = api::post_json::<_, Share>("/api/shares", &body).await {
                                            ui.say(err);
                                        }
                                        let _ = reload.try_update(|n| *n += 1);
                                    });
                                }
                            }>
                                <option value="view">{t("Can view")}</option>
                                <option value="edit">{t("Can edit")}</option>
                            </select>
                            <button type="button" class="icon-button danger" title=t("Remove access") aria-label=t("Remove access") on:click=move |_| revoke(share.id, reload)>{icon(Icon::Close)}</button>
                        </li>
                    </For>
                </ul>
                <h3>{t("Links")}</h3>
                <ul class="access">
                    <For each=link_rows key=|s| s.id let(share)>
                        <li>
                            <span class="avatar link">{icon(Icon::Link)}</span>
                            <input class="field link-field" readonly prop:value=share.token.as_deref().map(link_of).unwrap_or_default() />
                            <span class="badge">{if share.permission == "edit" { t("Can edit") } else { t("Can view") }}{if share.has_password { " · " } else { "" }}{if share.has_password { t("password") } else { "" }}</span>
                            <button type="button" class="button" on:click={
                                let token = share.token.clone().unwrap_or_default();
                                move |_| {
                                    copy_text(&link_of(&token));
                                    ui.say(t("Link copied."));
                                }
                            }>{t("Copy")}</button>
                            <button type="button" class="icon-button danger" title=t("Delete link") aria-label=t("Delete link") on:click=move |_| revoke(share.id, reload)>{icon(Icon::Trash)}</button>
                        </li>
                    </For>
                </ul>
                <div class="link-form">
                    <select class="field narrow" aria-label=t("Link permission") on:change=move |ev| link_permission.set(event_target_value(&ev)) prop:value=move || link_permission.get()>
                        <option value="view">{t("Anyone with the link can view")}</option>
                        <option value="edit">{t("Anyone with the link can edit")}</option>
                    </select>
                    <input class="field" type="password" placeholder=t("Password (optional)") autocomplete="new-password" prop:value=move || link_password.get() on:input=move |ev| link_password.set(event_target_value(&ev)) />
                    <label class="inline-label">{t("Expires")}<input class="field narrow" type="date" prop:value=move || link_expiry.get() on:input=move |ev| link_expiry.set(event_target_value(&ev)) /></label>
                    <button type="button" class="button" on:click=make_link>{icon(Icon::Link)}<span>{t("Create link")}</span></button>
                </div>
            </div>
        </Dialog>
    }
}

fn revoke(id: i64, reload: RwSignal<u32>) {
    let ui = ui();
    wasm_bindgen_futures::spawn_local(async move {
        if let Err(err) = api::delete(&format!("/api/shares/{id}")).await {
            ui.say(err);
        }
        let _ = reload.try_update(|n| *n += 1);
        ui.refresh();
    });
}

pub fn initials(name: &str) -> String {
    let letters: String = name
        .split_whitespace()
        .filter_map(|w| w.chars().next())
        .take(2)
        .collect();
    if letters.is_empty() {
        "?".into()
    } else {
        letters.to_uppercase()
    }
}

fn link_of(token: &str) -> String {
    let origin = web_sys::window()
        .and_then(|w| w.location().origin().ok())
        .unwrap_or_default();
    format!("{origin}/s/{token}")
}

fn copy_text(text: &str) {
    if let Some(window) = web_sys::window() {
        let _ = window.navigator().clipboard().write_text(text);
    }
}

/// `2026-12-31` → end of that day, local time, in Unix seconds.
fn parse_date(value: &str) -> Option<i64> {
    if value.trim().is_empty() {
        return None;
    }
    #[cfg(target_arch = "wasm32")]
    {
        let ms = js_sys::Date::new(&wasm_bindgen::JsValue::from_str(&format!(
            "{value}T23:59:59"
        )))
        .get_time();
        (!ms.is_nan()).then(|| (ms / 1000.0) as i64)
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        None
    }
}

/// Choose which app opens a file.
#[component]
fn OpenWithDialog(path: String, share: Option<i64>) -> impl IntoView {
    let ui = ui();
    let name = file_name(&path);
    let choices = apps_for(&name, &ui.apps.get_untracked());
    let download = match share {
        None => api::home_content(&path),
        Some(id) => api::share_content(id, &path),
    };
    let unknown = app_of_name(&name).is_none();
    view! {
        <Dialog title=tf("Open {name} with", &[("name", &name)])>
            <div class="dialog-body">
                {unknown.then(|| view! { <p class="muted">{t("No app opens this kind of file by its name. Pick one, or download it.")}</p> })}
                <ul class="app-choices">
                    {choices.into_iter().map(|(id, title, installed, best)| {
                        let path = path.clone();
                        let blurb = app_by_id(&id).map(|a| a.blurb).unwrap_or("");
                        view! {
                            <li><button type="button" class="app-choice" disabled=!installed on:click={
                                let id = id.clone();
                                move |_| {
                                    ui.close_modal();
                                    ui.from_drive.set(true);
                                    ui.go(Route::Open { share, path: path.clone(), app: Some(id.clone()) });
                                }
                            }>
                                {app_icon(&id, "card-icon")}
                                <span class="who"><strong>{title}</strong><span class="muted">{if installed { blurb.to_string() } else { t("Not installed yet").to_string() }}</span></span>
                                {best.then(|| view! { <span class="badge">{t("Suggested")}</span> })}
                            </button></li>
                        }
                    }).collect_view()}
                </ul>
                <footer>
                    <a class="button" href=download download=name.clone()>{icon(Icon::Download)}<span>{t("Download")}</span></a>
                </footer>
            </div>
        </Dialog>
    }
}

/// The app asked for a file: pick one from the drive (or this device).
#[component]
fn PickDialog(id: u32, accept: String, multiple: bool) -> impl IntoView {
    let ui = ui();
    let editor = ui.editor;
    let folder = RwSignal::new(editor.folder.get_untracked());
    let entries = RwSignal::new(Vec::<FileEntry>::new());
    let suggested = editor.suggested.get_untracked();
    let accept_list = accept.clone();
    Effect::new(move |_| {
        let at = folder.get();
        let accept = accept_list.clone();
        wasm_bindgen_futures::spawn_local(async move {
            match api::list_dir(&at).await {
                Ok(listing) => {
                    let mut list: Vec<FileEntry> = listing
                        .entries
                        .into_iter()
                        .filter(|e| e.is_dir || picker::accepts(&accept, &e.name))
                        .collect();
                    list.sort_by(|a, b| {
                        b.is_dir
                            .cmp(&a.is_dir)
                            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
                    });
                    let _ = entries.try_set(list);
                }
                Err(err) => ui.say(err),
            }
        });
    });
    let accept_doc = accept.to_ascii_lowercase();
    let choose = move |path: String| {
        let accept = accept_doc.clone();
        let app = editor.app.get_untracked();
        let name = file_name(&path);
        // File ▸ Open of a document for this same app: open it from the drive, so saving
        // writes back to it (an in-memory copy would save next to the current document).
        let doc_open = !multiple
            && app != "lightcraft"
            && app_by_id(&app).is_some_and(|a| {
                a.extensions
                    .iter()
                    .any(|ext| accept.to_ascii_lowercase().contains(&format!(".{ext}")))
                    && name.rsplit_once('.').is_some_and(|(_, ext)| {
                        a.extensions.contains(&ext.to_ascii_lowercase().as_str())
                    })
            });
        ui.close_modal();
        if doc_open {
            picker::dismiss(id);
            ui.from_drive.set(ui.from_drive.get_untracked());
            crate::editor::leave_doc(Some(Route::Open {
                share: None,
                path,
                app: Some(app),
            }));
            return;
        }
        wasm_bindgen_futures::spawn_local(async move {
            match api::send("GET", &api::home_content(&path), None, None, None).await {
                Ok(reply) if reply.status < 400 => picker::answer(id, vec![(name, reply.body)]),
                Ok(reply) => {
                    picker::dismiss(id);
                    ui.say(String::from_utf8_lossy(&reply.body).to_string());
                }
                Err(err) => {
                    picker::dismiss(id);
                    ui.say(err);
                }
            }
        });
    };
    let choose_suggested = choose.clone();
    let choose_entry = choose.clone();
    let device_accept = accept.clone();
    view! {
        <Dialog title=t("Choose a file").into() wide=true on_close=Callback::new(move |_| picker::dismiss(id))>
            <div class="dialog-body">
                {suggested.map(|path| {
                    let label = file_name(&path);
                    let choose = choose_suggested.clone();
                    view! {
                        <button type="button" class="suggested" on:click=move |_| choose(path.clone())>
                            {app_icon("lightcraft", "app-icon")}<span><strong>{label}</strong><span class="muted">" · suggested"</span></span>
                        </button>
                    }
                })}
                <div class="picker-path">
                    <button type="button" class="crumb" on:click=move |_| folder.set(String::new())>{icon(Icon::Home)}<span>{t("My files")}</span></button>
                    {move || {
                        let mut acc = String::new();
                        folder.get().split('/').filter(|s| !s.is_empty()).map(|segment| {
                            acc = if acc.is_empty() { segment.to_string() } else { format!("{acc}/{segment}") };
                            let target = acc.clone();
                            view! { <span class="sep">{icon(Icon::Chevron)}</span><button type="button" class="crumb" on:click=move |_| folder.set(target.clone())>{segment.to_string()}</button> }
                        }).collect_view()
                    }}
                </div>
                <ul class="picker-list tall">
                    <For each=move || entries.get() key=|e| format!("{}:{}", e.is_dir, e.path) let(entry)>
                        <li><button type="button" class="file-name" on:click={
                            let choose = choose_entry.clone();
                            let entry = entry.clone();
                            move |_| if entry.is_dir { folder.set(entry.path.clone()) } else { choose(entry.path.clone()) }
                        }>
                            <span class="file-icon">{if entry.is_dir { icon(Icon::Folder).into_any() } else { match app_of_name(&entry.name) { Some(app) => app_icon(app, "app-icon").into_any(), None => icon(Icon::File).into_any() } }}</span>
                            <span class="label">{entry.name.clone()}</span>
                        </button></li>
                    </For>
                    <Show when=move || entries.get().is_empty()><li class="empty">{t("No matching files in this folder.")}</li></Show>
                </ul>
                <footer>
                    <label class="button">
                        {icon(Icon::Upload)}<span>{t("From this device…")}</span>
                        <input type="file" class="hidden" accept=device_accept multiple=multiple on:change=move |ev| {
                            let input = crate::drive::event_target::<web_sys::HtmlInputElement>(&ev);
                            let Some(files) = input.files() else { return };
                            ui.close_modal();
                            wasm_bindgen_futures::spawn_local(async move {
                                let mut picked = Vec::new();
                                for index in 0..files.length() {
                                    let Some(file) = files.item(index) else { continue };
                                    let Ok(buffer) = wasm_bindgen_futures::JsFuture::from(file.array_buffer()).await else { continue };
                                    picked.push((file.name(), js_sys::Uint8Array::new(&buffer).to_vec()));
                                }
                                picker::answer(id, picked);
                            });
                        } />
                    </label>
                    <button type="button" class="button" on:click=move |_| {
                        picker::dismiss(id);
                        ui.close_modal();
                    }>{t("Cancel")}</button>
                </footer>
            </div>
        </Dialog>
    }
}
