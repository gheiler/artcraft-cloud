use artcraft_cloud_types::{app_for_name, AppStatus, FileEntry, Share, TrashEntry};
use leptos::prelude::*;
use wasm_bindgen::JsCast;

use crate::api;
use crate::editor::{file_name, parent_folder, Editor, SaveKind};

#[derive(Clone, PartialEq)]
pub enum Page {
    Files(String),
    Shared,
    Share(i64, String),
    Trash,
    Users,
    Public(String),
}

#[component]
pub fn Drive(
    page: RwSignal<Page>,
    editor: Editor,
    toast: RwSignal<String>,
    apps: RwSignal<Vec<AppStatus>>,
    refresh_tick: RwSignal<u32>,
) -> impl IntoView {
    let entries = RwSignal::new(Vec::<FileEntry>::new());
    let shares_in = RwSignal::new(Vec::<Share>::new());
    let shares_out = RwSignal::new(Vec::<Share>::new());
    let trash = RwSignal::new(Vec::<TrashEntry>::new());
    let public_needs_password = RwSignal::new(false);
    let public_name = RwSignal::new(String::new());
    let public_permission = RwSignal::new(String::new());
    let busy = RwSignal::new(false);

    Effect::new(move |_| {
        refresh_tick.get();
        let current = page.get();
        wasm_bindgen_futures::spawn_local(async move {
            busy.set(true);
            let result = load(current, entries, shares_in, shares_out, trash, public_needs_password, public_name, public_permission, apps).await;
            if let Err(err) = result {
                toast.set(err);
            }
            busy.set(false);
        });
    });

    let refresh = move || refresh_tick.update(|n| *n += 1);

    view! {
        <div class="drive"
            on:dragover=move |ev| ev.prevent_default()
            on:drop=move |ev| {
                ev.prevent_default();
                let Some(files) = ev.data_transfer().and_then(|dt| dt.files()) else { return };
                let folder = folder_of(page.get_untracked());
                wasm_bindgen_futures::spawn_local(async move {
                    for index in 0..files.length() {
                        let Some(file) = files.item(index) else { continue };
                        let buffer = wasm_bindgen_futures::JsFuture::from(file.array_buffer()).await;
                        let Ok(buffer) = buffer else { continue };
                        let bytes = js_sys::Uint8Array::new(&buffer).to_vec();
                        if let Err(err) = api::upload(&folder, &file.name(), &bytes).await {
                            toast.set(err);
                        }
                    }
                    refresh();
                });
            }
        >
            <div class="toolbar">
                <div class="crumbs">{move || crumbs(page.get())}</div>
                <div class="tools">
                    <Show when=move || matches!(page.get(), Page::Files(_))>
                        <label class="button">
                            "Upload"
                            <input type="file" class="hidden" multiple on:change=move |ev| {
                                let input = event_target::<web_sys::HtmlInputElement>(&ev);
                                let Some(files) = input.files() else { return };
                                let folder = folder_of(page.get_untracked());
                                wasm_bindgen_futures::spawn_local(async move {
                                    for index in 0..files.length() {
                                        let Some(file) = files.item(index) else { continue };
                                        let Ok(buffer) = wasm_bindgen_futures::JsFuture::from(file.array_buffer()).await else { continue };
                                        let bytes = js_sys::Uint8Array::new(&buffer).to_vec();
                                        if let Err(err) = api::upload(&folder, &file.name(), &bytes).await {
                                            toast.set(err);
                                        }
                                    }
                                    refresh();
                                });
                            } />
                        </label>
                        <button type="button" on:click=move |_| mkdir(page, toast, refresh_tick)>"New folder"</button>
                    </Show>
                    <Show when=move || busy.get()><span class="muted">"Working…"</span></Show>
                </div>
            </div>
            <Show when=move || public_needs_password.get()>
                <PublicUnlock page=page toast=toast tick=refresh_tick />
            </Show>
            <Show when=move || matches!(page.get(), Page::Shared)>
                <ShareLists incoming=shares_in outgoing=shares_out page=page toast=toast tick=refresh_tick />
            </Show>
            <Show when=move || matches!(page.get(), Page::Trash)>
                <TrashList rows=trash toast=toast tick=refresh_tick />
            </Show>
            <Show when=move || matches!(page.get(), Page::Files(_) | Page::Share(_, _) | Page::Public(_)) && !public_needs_password.get()>
                <FileTable
                    entries=entries
                    page=page
                    editor=editor
                    toast=toast
                    apps=apps
                    tick=refresh_tick
                    permission=public_permission
                />
            </Show>
            <div class="launch">
                <span class="muted">"New"</span>
                <For each=move || apps.get() key=|app| app.id.clone() let(app)>
                    <button type="button" disabled=move || !app.installed on:click={
                        let id = app.id.clone();
                        let title = app.title.clone();
                        let ext = app.extensions.first().cloned().unwrap_or_else(|| "txt".into());
                        move |_| {
                            let folder = folder_of(page.get_untracked());
                            editor.launch(&id, &title, format!("Untitled.{ext}"), None, None, folder, SaveKind::Home, false, None);
                        }
                    }>{app.title.clone()}</button>
                </For>
            </div>
        </div>
    }
}

#[allow(clippy::too_many_arguments)]
async fn load(
    page: Page,
    entries: RwSignal<Vec<FileEntry>>,
    shares_in: RwSignal<Vec<Share>>,
    shares_out: RwSignal<Vec<Share>>,
    trash: RwSignal<Vec<TrashEntry>>,
    needs_password: RwSignal<bool>,
    public_name: RwSignal<String>,
    permission: RwSignal<String>,
    apps: RwSignal<Vec<AppStatus>>,
) -> Result<(), String> {
    if let Ok(list) = api::apps().await {
        apps.set(list);
    }
    match page {
        Page::Files(path) => {
            let listing = api::list_dir(&path).await?;
            entries.set(listing.entries);
            permission.set("edit".into());
        }
        Page::Share(id, path) => {
            let listing = api::get_json::<artcraft_cloud_types::DirListing>(&format!("/api/shares/{id}/list?path={}", api::enc(&path))).await?;
            entries.set(listing.entries);
            permission.set("edit".into());
        }
        Page::Public(token) => {
            #[derive(serde::Deserialize)]
            struct View {
                needs_password: bool,
                permission: String,
                name: String,
                listing: Option<artcraft_cloud_types::DirListing>,
                expired: bool,
            }
            let view: View = api::get_json(&format!("/api/public/{token}")).await?;
            if view.expired {
                return Err("this link has expired".into());
            }
            needs_password.set(view.needs_password);
            public_name.set(view.name);
            permission.set(view.permission);
            entries.set(view.listing.map(|listing| listing.entries).unwrap_or_default());
        }
        Page::Shared => {
            shares_in.set(api::shares(true).await?);
            shares_out.set(api::shares(false).await?);
        }
        Page::Trash => trash.set(api::trash().await?),
        Page::Users => {}
    }
    Ok(())
}

fn folder_of(page: Page) -> String {
    match page {
        Page::Files(path) => path,
        Page::Share(_, _) | Page::Public(_) => String::new(),
        _ => String::new(),
    }
}

fn crumbs(page: Page) -> String {
    match page {
        Page::Files(path) if path.is_empty() => "My files".into(),
        Page::Files(path) => format!("My files / {path}"),
        Page::Shared => "Shared".into(),
        Page::Share(id, path) => format!("Share {id} / {path}"),
        Page::Trash => "Trash".into(),
        Page::Users => "People".into(),
        Page::Public(_) => "Shared link".into(),
    }
}

#[component]
fn FileTable(
    entries: RwSignal<Vec<FileEntry>>,
    page: RwSignal<Page>,
    editor: Editor,
    toast: RwSignal<String>,
    apps: RwSignal<Vec<AppStatus>>,
    tick: RwSignal<u32>,
    permission: RwSignal<String>,
) -> impl IntoView {
    view! {
        <table>
            <thead><tr><th>"Name"</th><th>"Size"</th><th></th></tr></thead>
            <tbody>
                <Show when=move || matches!(page.get(), Page::Files(ref path) if !path.is_empty()) || matches!(page.get(), Page::Share(_, ref path) if !path.is_empty())>
                    <tr class="row">
                        <td colspan="3"><button type="button" on:click=move |_| up(page)>".."</button></td>
                    </tr>
                </Show>
                <For each=move || entries.get() key=|entry| format!("{}:{}", entry.is_dir, entry.path) let(entry)>
                    <FileRow entry=entry page=page editor=editor toast=toast apps=apps tick=tick permission=permission />
                </For>
            </tbody>
        </table>
    }
}

#[component]
fn FileRow(
    entry: FileEntry,
    page: RwSignal<Page>,
    editor: Editor,
    toast: RwSignal<String>,
    apps: RwSignal<Vec<AppStatus>>,
    tick: RwSignal<u32>,
    permission: RwSignal<String>,
) -> impl IntoView {
    let path = entry.path.clone();
    let name = entry.name.clone();
    let is_dir = entry.is_dir;
    let size = entry.size;
    let opened = entry.clone();
    let download = path.clone();
    let share = path.clone();
    let rename = path.clone();
    let move_to = path.clone();
    let copy = path.clone();
    let trash = path;
    view! {
        <tr class="row">
            <td><button type="button" class="link" on:click=move |_| open_entry(opened.clone(), page, editor, apps, toast)>{move || name.clone()}</button></td>
            <td class="muted">{move || if is_dir { "folder".into() } else { format_size(size) }}</td>
            <td class="row-actions">
                {move || {
                    let download = download.clone();
                    let share = share.clone();
                    let rename = rename.clone();
                    let move_to = move_to.clone();
                    let copy = copy.clone();
                    let trash = trash.clone();
                    let can_edit = permission.get() != "view" && matches!(page.get(), Page::Files(_));
                    view! {
                        <span>
                            {(!is_dir).then(|| view! { <a href=download_href(&page.get_untracked(), &download)>"Download"</a> })}
                            {can_edit.then(|| view! {
                                <span>
                                    <button type="button" on:click=move |_| share_dialog(&share, toast, tick)>"Share"</button>
                                    <button type="button" on:click=move |_| rename_dialog(&rename, toast, tick)>"Rename"</button>
                                    <button type="button" on:click=move |_| move_dialog(&move_to, toast, tick)>"Move"</button>
                                    <button type="button" on:click=move |_| copy_dialog(&copy, toast, tick)>"Copy"</button>
                                    <button type="button" class="danger" on:click=move |_| {
                                        let path = trash.clone();
                                        wasm_bindgen_futures::spawn_local(async move {
                                            if let Err(err) = api::post_json::<_, serde_json::Value>("/api/files/delete", &serde_json::json!({"path": path})).await {
                                                toast.set(err);
                                            }
                                            tick.update(|n| *n += 1);
                                        });
                                    }>"Trash"</button>
                                </span>
                            })}
                        </span>
                    }
                }}
            </td>
        </tr>
    }
}

fn up(page: RwSignal<Page>) {
    page.update(|current| match current {
        Page::Files(path) => *current = Page::Files(parent_folder(path)),
        Page::Share(id, path) => *current = Page::Share(*id, parent_folder(path)),
        _ => {}
    });
}

fn open_entry(entry: FileEntry, page: RwSignal<Page>, editor: Editor, apps: RwSignal<Vec<AppStatus>>, toast: RwSignal<String>) {
    if entry.is_dir {
        page.update(|current| match current {
            Page::Files(_) => *current = Page::Files(entry.path.clone()),
            Page::Share(id, _) => *current = Page::Share(*id, entry.path.clone()),
            Page::Public(_) => {}
            _ => {}
        });
        return;
    }
    let Some(app) = app_for_name(&entry.name) else {
        toast.set(format!("No app for .{}", entry.name.rsplit('.').next().unwrap_or("")));
        return;
    };
    let installed = apps.with_untracked(|list| list.iter().any(|item| item.id == app.id && item.installed));
    if !installed {
        toast.set(format!("{} is not installed yet. It downloads in the background after startup.", app.title));
        return;
    }
    let current = page.get_untracked();
    let (url, kind, folder, read_only) = match &current {
        Page::Files(_) => (api::home_content(&entry.path), SaveKind::Home, parent_folder(&entry.path), false),
        Page::Share(id, _) => (api::share_content(*id, &entry.path), SaveKind::Share(*id), parent_folder(&entry.path), false),
        Page::Public(token) => (api::public_content(token, &entry.path), SaveKind::Public(token.clone()), parent_folder(&entry.path), false),
        _ => return,
    };
    let app_id = app.id.to_string();
    let title = app.title.to_string();
    wasm_bindgen_futures::spawn_local(async move {
        match api::send("GET", &url, None, None, None).await {
            Ok(reply) if reply.status < 400 => {
                editor.launch(&app_id, &title, file_name(&entry.path), Some(entry.path), reply.etag, folder, kind, read_only, Some(reply.body));
            }
            Ok(reply) => toast.set(String::from_utf8_lossy(&reply.body).to_string()),
            Err(err) => toast.set(err),
        }
    });
}

fn download_href(page: &Page, path: &str) -> String {
    match page {
        Page::Files(_) => api::home_content(path),
        Page::Share(id, _) => api::share_content(*id, path),
        Page::Public(token) => api::public_content(token, path),
        _ => "#".into(),
    }
}

fn prompt(message: &str) -> Option<String> {
    web_sys::window().and_then(|window| window.prompt_with_message(message).ok()).flatten().filter(|value| !value.trim().is_empty())
}

fn mkdir(page: RwSignal<Page>, toast: RwSignal<String>, tick: RwSignal<u32>) {
    let Some(name) = prompt("Folder name") else { return };
    let Page::Files(dir) = page.get_untracked() else { return };
    let path = if dir.is_empty() { name } else { format!("{dir}/{name}") };
    wasm_bindgen_futures::spawn_local(async move {
        if let Err(err) = api::post_json::<_, serde_json::Value>("/api/files/mkdir", &serde_json::json!({"path": path})).await {
            toast.set(err);
        }
        tick.update(|n| *n += 1);
    });
}

fn rename_dialog(path: &str, toast: RwSignal<String>, tick: RwSignal<u32>) {
    let Some(name) = prompt("New name") else { return };
    let path = path.to_string();
    wasm_bindgen_futures::spawn_local(async move {
        if let Err(err) = api::post_json::<_, serde_json::Value>("/api/files/rename", &serde_json::json!({"path": path, "name": name})).await {
            toast.set(err);
        }
        tick.update(|n| *n += 1);
    });
}

fn move_dialog(path: &str, toast: RwSignal<String>, tick: RwSignal<u32>) {
    let Some(to_dir) = prompt("Move into folder (empty for the top)") else { return };
    let path = path.to_string();
    wasm_bindgen_futures::spawn_local(async move {
        if let Err(err) = api::post_json::<_, serde_json::Value>("/api/files/move", &serde_json::json!({"from": path, "to_dir": to_dir})).await {
            toast.set(err);
        }
        tick.update(|n| *n += 1);
    });
}

fn copy_dialog(path: &str, toast: RwSignal<String>, tick: RwSignal<u32>) {
    let Some(to_dir) = prompt("Copy into folder (empty for the top)") else { return };
    let path = path.to_string();
    wasm_bindgen_futures::spawn_local(async move {
        if let Err(err) = api::post_json::<_, serde_json::Value>("/api/files/copy", &serde_json::json!({"from": path, "to_dir": to_dir})).await {
            toast.set(err);
        }
        tick.update(|n| *n += 1);
    });
}

fn share_dialog(path: &str, toast: RwSignal<String>, tick: RwSignal<u32>) {
    let person = prompt("Username to share with (leave empty for a public link)").unwrap_or_default();
    let permission = prompt("Permission: view or edit").unwrap_or_else(|| "view".into());
    let permission = if permission == "edit" { "edit" } else { "view" };
    let public = person.trim().is_empty();
    let path = path.to_string();
    wasm_bindgen_futures::spawn_local(async move {
        let body = serde_json::json!({
            "path": path,
            "grantee": if public { serde_json::Value::Null } else { serde_json::Value::String(person) },
            "permission": permission,
            "public": public,
        });
        match api::post_json::<_, Share>("/api/shares", &body).await {
            Ok(share) => {
                if let Some(token) = share.token {
                    let origin = web_sys::window().and_then(|w| w.location().origin().ok()).unwrap_or_default();
                    toast.set(format!("Link: {origin}/s/{token}"));
                } else {
                    toast.set("Shared.".into());
                }
            }
            Err(err) => toast.set(err),
        }
        tick.update(|n| *n += 1);
    });
}

#[component]
fn ShareLists(incoming: RwSignal<Vec<Share>>, outgoing: RwSignal<Vec<Share>>, page: RwSignal<Page>, toast: RwSignal<String>, tick: RwSignal<u32>) -> impl IntoView {
    view! {
        <section>
            <h2>"Shared with me"</h2>
            <ul>
                <For each=move || incoming.get() key=|share| share.id let(share)>
                    <li>
                        <button type="button" class="link" on:click=move |_| page.set(Page::Share(share.id, String::new()))>
                            {share.owner.clone()}" / "{share.path.clone()}" ("{share.permission.clone()}")"
                        </button>
                    </li>
                </For>
            </ul>
            <h2>"Shared by me"</h2>
            <ul>
                <For each=move || outgoing.get() key=|share| share.id let(share)>
                    <li>
                        {share.path.clone()}
                        " → "
                        {share.grantee.clone().unwrap_or_else(|| "public link".into())}
                        " ("{share.permission.clone()}") "
                        <button type="button" class="danger" on:click=move |_| {
                            let id = share.id;
                            wasm_bindgen_futures::spawn_local(async move {
                                if let Err(err) = api::send("DELETE", &format!("/api/shares/{id}"), None, None, None).await {
                                    toast.set(err);
                                }
                                tick.update(|n| *n += 1);
                            });
                        }>"Revoke"</button>
                    </li>
                </For>
            </ul>
        </section>
    }
}

#[component]
fn TrashList(rows: RwSignal<Vec<TrashEntry>>, toast: RwSignal<String>, tick: RwSignal<u32>) -> impl IntoView {
    view! {
        <table>
            <tbody>
                <For each=move || rows.get() key=|row| row.id.clone() let(row)>
                    <tr>
                        <td>{row.original_path.clone()}</td>
                        <td>
                            <button type="button" on:click={
                                let id = row.id.clone();
                                move |_| {
                                    let id = id.clone();
                                    wasm_bindgen_futures::spawn_local(async move {
                                        if let Err(err) = api::post_empty(&format!("/api/trash/{id}/restore")).await {
                                            toast.set(err);
                                        }
                                        tick.update(|n| *n += 1);
                                    });
                                }
                            }>"Restore"</button>
                            <button type="button" class="danger" on:click={
                                let id = row.id.clone();
                                move |_| {
                                    let id = id.clone();
                                    wasm_bindgen_futures::spawn_local(async move {
                                        if let Err(err) = api::send("DELETE", &format!("/api/trash/{id}"), None, None, None).await {
                                            toast.set(err);
                                        }
                                        tick.update(|n| *n += 1);
                                    });
                                }
                            }>"Delete forever"</button>
                        </td>
                    </tr>
                </For>
            </tbody>
        </table>
        <button type="button" on:click=move |_| {
            wasm_bindgen_futures::spawn_local(async move {
                if let Err(err) = api::send("DELETE", "/api/trash", None, None, None).await {
                    toast.set(err);
                }
                tick.update(|n| *n += 1);
            });
        }>"Empty trash"</button>
    }
}

#[component]
fn PublicUnlock(page: RwSignal<Page>, toast: RwSignal<String>, tick: RwSignal<u32>) -> impl IntoView {
    let password = RwSignal::new(String::new());
    view! {
        <form class="unlock" on:submit=move |ev| {
            ev.prevent_default();
            let Page::Public(token) = page.get_untracked() else { return };
            let password = password.get_untracked();
            wasm_bindgen_futures::spawn_local(async move {
                match api::post_json::<_, serde_json::Value>(&format!("/api/public/{token}/unlock"), &serde_json::json!({"password": password})).await {
                    Ok(_) => tick.update(|n| *n += 1),
                    Err(err) => toast.set(err),
                }
            });
        }>
            <label>"This link has a password"</label>
            <input type="password" prop:value=move || password.get() on:input=move |ev| password.set(event_target_value(&ev)) />
            <button type="submit">"Unlock"</button>
        </form>
    }
}

fn format_size(size: u64) -> String {
    if size < 1024 {
        format!("{size} B")
    } else if size < 1024 * 1024 {
        format!("{} KB", size / 1024)
    } else {
        format!("{:.1} MB", size as f64 / 1024.0 / 1024.0)
    }
}

fn event_target<T: JsCast>(ev: &web_sys::Event) -> T {
    ev.target().and_then(|target| target.dyn_into().ok()).expect("event target")
}
