//! The drive: folders, files, shares and the trash.

use crate::i18n::{t, tf};
use artcraft_cloud_types::{AppStatus, FileEntry, Share, TrashEntry};
use leptos::prelude::*;
use wasm_bindgen::JsCast;

use crate::api;
use crate::catalog::{app_by_id, app_for_name, catalog};
use crate::editor::{file_name, parent_folder, Launch, SaveKind};
use crate::icons::{app_icon, icon, Icon};
use crate::route::Route;
use crate::ui::{ui, Modal};

/// Which app a file belongs to for its icon and the filter. LightCraft backups are zips with a
/// recognisable name; their content is checked when they are opened.
pub fn app_of_name(name: &str) -> Option<&'static str> {
    if let Some(app) = app_for_name(name) {
        return Some(app.id);
    }
    let lower = name.to_ascii_lowercase();
    (lower.ends_with(".zip") && lower.contains("lightcraft")).then_some("lightcraft")
}

#[component]
pub fn Drive() -> impl IntoView {
    let ui = ui();
    let entries = RwSignal::new(Vec::<FileEntry>::new());
    let shares_in = RwSignal::new(Vec::<Share>::new());
    let shares_out = RwSignal::new(Vec::<Share>::new());
    let trash = RwSignal::new(Vec::<TrashEntry>::new());
    let public_needs_password = RwSignal::new(false);
    let public_permission = RwSignal::new(String::from("edit"));
    let busy = RwSignal::new(false);
    let filter = RwSignal::new(None::<String>);

    Effect::new(move |_| {
        ui.tick.get();
        let current = ui.route.get();
        if current.is_doc() {
            return;
        }
        wasm_bindgen_futures::spawn_local(async move {
            let _ = busy.try_set(true);
            let result = load(
                current,
                entries,
                shares_in,
                shares_out,
                trash,
                public_needs_password,
                public_permission,
            )
            .await;
            if let Err(err) = result {
                ui.say(err);
            }
            let _ = busy.try_set(false);
        });
    });

    view! {
        <div class="drive"
            on:dragover=move |ev| ev.prevent_default()
            on:drop=move |ev| {
                ev.prevent_default();
                let Some(files) = ev.data_transfer().and_then(|dt| dt.files()) else { return };
                if let Route::Files(folder) = ui.route.get_untracked() {
                    upload_files(files, folder);
                }
            }
        >
            <header class="toolbar">
                <Crumbs />
                <div class="tools">
                    <Show when=move || busy.get()><span class="spinner" aria-label=t("Loading")></span></Show>
                    <Show when=move || matches!(ui.route.get(), Route::Files(_))>
                        <label class="button">
                            {icon(Icon::Upload)}<span>{t("Upload")}</span>
                            <input type="file" class="hidden" multiple on:change=move |ev| {
                                let input = event_target::<web_sys::HtmlInputElement>(&ev);
                                let Some(files) = input.files() else { return };
                                if let Route::Files(folder) = ui.route.get_untracked() {
                                    upload_files(files, folder);
                                }
                                input.set_value("");
                            } />
                        </label>
                        <button type="button" class="button" on:click=move |_| {
                            if let Route::Files(dir) = ui.route.get_untracked() {
                                ui.open_modal(Modal::NewFolder { dir });
                            }
                        }>{icon(Icon::FolderPlus)}<span>{t("New folder")}</span></button>
                    </Show>
                </div>
            </header>
            <Show when=move || matches!(ui.route.get(), Route::Files(_))>
                <NewCards />
            </Show>
            <Show when=move || public_needs_password.try_get().unwrap_or(false)>
                <PublicUnlock />
            </Show>
            <Show when=move || matches!(ui.route.get(), Route::Shared)>
                <ShareLists incoming=shares_in outgoing=shares_out />
            </Show>
            <Show when=move || matches!(ui.route.get(), Route::Trash)>
                <TrashList rows=trash />
            </Show>
            <Show when=move || matches!(ui.route.get(), Route::Files(_) | Route::Share(_, _) | Route::Public(_)) && !public_needs_password.try_get().unwrap_or(true)>
                <section class="card files">
                    <div class="files-head">
                        <h2>{move || if matches!(ui.route.get(), Route::Files(_)) { t("Files") } else { t("Shared files") }}</h2>
                        <Filters entries=entries filter=filter />
                    </div>
                    <FileTable entries=entries permission=public_permission filter=filter />
                </section>
            </Show>
        </div>
    }
}

fn upload_files(files: web_sys::FileList, folder: String) {
    let ui = ui();
    wasm_bindgen_futures::spawn_local(async move {
        for index in 0..files.length() {
            let Some(file) = files.item(index) else {
                continue;
            };
            let Ok(buffer) = wasm_bindgen_futures::JsFuture::from(file.array_buffer()).await else {
                continue;
            };
            let bytes = js_sys::Uint8Array::new(&buffer).to_vec();
            if let Err(err) = api::upload(&folder, &file.name(), &bytes).await {
                ui.say(err);
            }
        }
        ui.refresh();
    });
}

#[component]
fn Crumbs() -> impl IntoView {
    let ui = ui();
    move || {
        let route = ui.route.get();
        let (root, root_route, folder, share) = match &route {
            Route::Files(folder) => (
                t("My files"),
                Route::Files(String::new()),
                folder.clone(),
                None,
            ),
            Route::Share(id, folder) => (t("Shared"), Route::Shared, folder.clone(), Some(*id)),
            Route::Shared => (t("Shared"), Route::Shared, String::new(), None),
            Route::Trash => (t("Trash"), Route::Trash, String::new(), None),
            Route::People => (t("People"), Route::People, String::new(), None),
            Route::Settings => (t("Settings"), Route::Settings, String::new(), None),
            Route::Public(_) => (t("Shared link"), route.clone(), String::new(), None),
            _ => (
                t("My files"),
                Route::Files(String::new()),
                String::new(),
                None,
            ),
        };
        let mut parts = Vec::new();
        let mut acc = String::new();
        if let Some(id) = share {
            parts.push((t("Share").to_string(), Route::Share(id, String::new())));
        }
        for segment in folder.split('/').filter(|s| !s.is_empty()) {
            acc = if acc.is_empty() {
                segment.to_string()
            } else {
                format!("{acc}/{segment}")
            };
            let target = match share {
                Some(id) => Route::Share(id, acc.clone()),
                None => Route::Files(acc.clone()),
            };
            parts.push((segment.to_string(), target));
        }
        view! {
            <nav class="crumbs" aria-label=t("Folder")>
                <button type="button" class="crumb" on:click=move |_| ui.go(root_route.clone())>{root}</button>
                {parts.into_iter().map(|(label, target)| view! {
                    <span class="sep">{icon(Icon::Chevron)}</span>
                    <button type="button" class="crumb" on:click=move |_| ui.go(target.clone())>{label}</button>
                }).collect_view()}
            </nav>
        }
    }
}

/// "New" cards, one per app, with the app's own icon.
#[component]
fn NewCards() -> impl IntoView {
    let ui = ui();
    view! {
        <section class="new-cards" aria-label=t("New document")>
            <For each=move || ui.apps.get() key=|app| (app.id.clone(), app.installed) let(app)>
                <button type="button" class="new-card" disabled=!app.installed
                    title=if !app.installed { tf("{app} is still downloading", &[("app", &app.title)]) } else if app_by_id(&app.id).is_some_and(|a| !a.bridge) { tf("Open {app}", &[("app", &app.title)]) } else { tf("New {app} document", &[("app", &app.title)]) }
                    on:click={
                        let id = app.id.clone();
                        move |_| {
                            let Route::Files(folder) = ui.route.get_untracked() else { return };
                            ui.from_drive.set(true);
                            ui.go(Route::New { app: id.clone(), folder });
                        }
                    }>
                    {app_icon(&app.id, "card-icon")}
                    <span class="card-title">{app.title.clone()}</span>
                    <span class="card-blurb">{if app.installed { app.blurb.clone() } else { t("Downloading…").to_string() }}</span>
                </button>
            </For>
        </section>
    }
}

/// Filter chips: all files, or the files of one app (only apps with files here are offered).
#[component]
fn Filters(entries: RwSignal<Vec<FileEntry>>, filter: RwSignal<Option<String>>) -> impl IntoView {
    let present = move || {
        let ids: Vec<&'static str> = entries.with(|list| {
            let mut ids: Vec<&'static str> = list
                .iter()
                .filter(|e| !e.is_dir)
                .filter_map(|e| app_of_name(&e.name))
                .collect();
            ids.sort_unstable();
            ids.dedup();
            ids
        });
        catalog()
            .iter()
            .filter(|app| ids.contains(&app.id))
            .map(|app| (app.id, app.title))
            .collect::<Vec<_>>()
    };
    view! {
        <div class="chips" role="group" aria-label=t("Filter by app")>
            <button type="button" class="chip" class:on=move || filter.get().is_none() on:click=move |_| filter.set(None)>{t("All")}</button>
            {move || present().into_iter().map(|(id, title)| view! {
                <button type="button" class="chip" class:on=move || filter.get().as_deref() == Some(id)
                    on:click=move |_| filter.set(if filter.get_untracked().as_deref() == Some(id) { None } else { Some(id.to_string()) })>
                    {app_icon(id, "chip-icon")}{title}
                </button>
            }).collect_view()}
        </div>
    }
}

#[allow(clippy::too_many_arguments)]
async fn load(
    route: Route,
    entries: RwSignal<Vec<FileEntry>>,
    shares_in: RwSignal<Vec<Share>>,
    shares_out: RwSignal<Vec<Share>>,
    trash: RwSignal<Vec<TrashEntry>>,
    needs_password: RwSignal<bool>,
    permission: RwSignal<String>,
) -> Result<(), String> {
    let ui = ui();
    if !matches!(route, Route::Public(_)) {
        if let Ok(list) = api::apps().await {
            crate::catalog::adopt(ui, list);
        }
    }
    match route {
        Route::Files(path) => {
            let listing = api::list_dir(&path).await?;
            let _ = entries.try_set(sorted(listing.entries));
            let _ = permission.try_set("edit".into());
            let _ = needs_password.try_set(false);
        }
        Route::Share(id, path) => {
            let listing = api::get_json::<artcraft_cloud_types::DirListing>(&format!(
                "/api/shares/{id}/list?path={}",
                api::enc(&path)
            ))
            .await?;
            let _ = entries.try_set(sorted(listing.entries));
            let _ = permission.try_set("edit".into());
            let _ = needs_password.try_set(false);
        }
        Route::Public(token) => {
            #[derive(serde::Deserialize)]
            struct View {
                needs_password: bool,
                permission: String,
                listing: Option<artcraft_cloud_types::DirListing>,
                expired: bool,
                name: String,
                is_dir: Option<bool>,
            }
            let view: View = api::get_json(&format!("/api/public/{token}")).await?;
            if view.expired {
                return Err(t("This link has expired.").into());
            }
            let _ = needs_password.try_set(view.needs_password);
            let _ = permission.try_set(view.permission);
            let mut list = view
                .listing
                .map(|listing| listing.entries)
                .unwrap_or_default();
            if list.is_empty()
                && !view.needs_password
                && view.is_dir != Some(true)
                && !view.name.is_empty()
            {
                list.push(FileEntry {
                    name: view.name.clone(),
                    path: String::new(),
                    is_dir: false,
                    size: 0,
                    etag: String::new(),
                    modified: 0,
                });
            }
            let _ = entries.try_set(list);
        }
        Route::Shared => {
            let _ = shares_in.try_set(api::shares(true).await?);
            let _ = shares_out.try_set(api::shares(false).await?);
        }
        Route::Trash => {
            let _ = trash.try_set(api::trash().await?);
        }
        Route::People
        | Route::Apps
        | Route::Mail
        | Route::Settings
        | Route::Open { .. }
        | Route::New { .. } => {}
    }
    Ok(())
}

fn sorted(mut entries: Vec<FileEntry>) -> Vec<FileEntry> {
    entries.sort_by(|a, b| {
        b.is_dir
            .cmp(&a.is_dir)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    entries
}

#[component]
fn FileTable(
    entries: RwSignal<Vec<FileEntry>>,
    permission: RwSignal<String>,
    filter: RwSignal<Option<String>>,
) -> impl IntoView {
    let ui = ui();
    let visible = move || {
        let filter = filter.get();
        entries
            .get()
            .into_iter()
            .filter(|e| {
                e.is_dir
                    || filter
                        .as_deref()
                        .is_none_or(|id| app_of_name(&e.name) == Some(id))
            })
            .collect::<Vec<_>>()
    };
    let up = move || match ui.route.get() {
        Route::Files(path) if !path.is_empty() => Some(Route::Files(parent_folder(&path))),
        Route::Share(id, path) if !path.is_empty() => Some(Route::Share(id, parent_folder(&path))),
        _ => None,
    };
    view! {
        <table class="file-table">
            <thead><tr><th class="name-col">{t("Name")}</th><th>{t("Modified")}</th><th>{t("Size")}</th><th class="actions-col"><span class="sr-only">{t("Actions")}</span></th></tr></thead>
            <tbody>
                {move || up().map(|target| view! {
                    <tr class="row up">
                        <td colspan="4"><button type="button" class="file-name" on:click=move |_| ui.go(target.clone())>
                            <span class="file-icon">{icon(Icon::Back)}</span><span>{t("Up one folder")}</span>
                        </button></td>
                    </tr>
                })}
                <For each=visible key=|entry| format!("{}:{}:{}", entry.is_dir, entry.path, entry.etag) let(entry)>
                    <FileRow entry=entry permission=permission />
                </For>
                <Show when=move || visible().is_empty()>
                    <tr><td colspan="4" class="empty">{move || if filter.get().is_some() { t("No files of that kind here.") } else { t("Nothing here yet. Drop files on this page or create a document above.") }}</td></tr>
                </Show>
            </tbody>
        </table>
    }
}

#[component]
fn FileRow(entry: FileEntry, permission: RwSignal<String>) -> impl IntoView {
    let ui = ui();
    let FileEntry {
        name,
        path,
        is_dir,
        size,
        modified,
        ..
    } = entry.clone();
    let app = if is_dir { None } else { app_of_name(&name) };
    let opened = entry.clone();
    // `permission` belongs to the drive, which may be gone by the time a route change reaches
    // this row: read it with try_get.
    let can_edit = move || {
        matches!(ui.route.get(), Route::Files(_))
            && permission.try_get().is_some_and(|p| p != "view")
    };
    let is_home = move || matches!(ui.route.get(), Route::Files(_));
    let download = download_href(&ui.route.get_untracked(), &path);
    let p = path.clone();
    let action = move |modal: fn(String) -> Modal| {
        let path = p.clone();
        move |_| ui.open_modal(modal(path.clone()))
    };
    let share_path = path.clone();
    let open_with_path = path.clone();
    view! {
        <tr class="row">
            <td class="name-col">
                <button type="button" class="file-name" on:click=move |_| open_entry(opened.clone())>
                    <span class="file-icon">{match (is_dir, app) {
                        (true, _) => icon(Icon::Folder).into_any(),
                        (false, Some(app)) => app_icon(app, "app-icon").into_any(),
                        (false, None) => icon(Icon::File).into_any(),
                    }}</span>
                    <span class="label">{name.clone()}</span>
                </button>
            </td>
            <td class="muted">{format_date(modified)}</td>
            <td class="muted">{if is_dir { "—".to_string() } else { format_size(size) }}</td>
            <td class="row-actions">
                {(!is_dir).then(|| view! {
                    <a class="icon-button" href=download.clone() download=name.clone() title=t("Download") aria-label=tf("Download {name}", &[("name", &name)])>{icon(Icon::Download)}</a>
                })}
                <Show when=move || !is_dir && (is_home() || matches!(ui.route.get(), Route::Share(_, _)))>
                    <button type="button" class="icon-button" title=t("Open with…") aria-label=t("Open with") on:click={
                        let path = open_with_path.clone();
                        move |_| {
                            let share = match ui.route.get_untracked() { Route::Share(id, _) => Some(id), _ => None };
                            ui.open_modal(Modal::OpenWith { path: path.clone(), share });
                        }
                    }>{icon(Icon::OpenWith)}</button>
                </Show>
                <Show when=can_edit>
                    <button type="button" class="icon-button" title=t("Share") aria-label=t("Share") on:click={
                        let path = share_path.clone();
                        move |_| ui.open_modal(Modal::Share { path: path.clone(), is_dir })
                    }>{icon(Icon::Share)}</button>
                    <button type="button" class="icon-button" title=t("Rename") aria-label=t("Rename") on:click=action(|path| Modal::Rename { path })>{icon(Icon::Rename)}</button>
                    <button type="button" class="icon-button" title=t("Move") aria-label=t("Move") on:click=action(|path| Modal::Transfer { path, copy: false })>{icon(Icon::Move)}</button>
                    <button type="button" class="icon-button" title=t("Copy") aria-label=t("Copy") on:click=action(|path| Modal::Transfer { path, copy: true })>{icon(Icon::Copy)}</button>
                    <button type="button" class="icon-button danger" title=t("Move to trash") aria-label=t("Move to trash") on:click=action(|path| Modal::Trash { path })>{icon(Icon::Trash)}</button>
                </Show>
            </td>
        </tr>
    }
}

fn open_entry(entry: FileEntry) {
    let ui = ui();
    let route = ui.route.get_untracked();
    if entry.is_dir {
        match route {
            Route::Files(_) => ui.go(Route::Files(entry.path)),
            Route::Share(id, _) => ui.go(Route::Share(id, entry.path)),
            _ => {}
        }
        return;
    }
    ui.from_drive.set(true);
    match route {
        Route::Files(_) => ui.go(Route::Open {
            share: None,
            path: entry.path,
            app: None,
        }),
        Route::Share(id, _) => ui.go(Route::Open {
            share: Some(id),
            path: entry.path,
            app: None,
        }),
        Route::Public(token) => open_file(SaveKind::Public(token), entry.path, entry.name, None),
        _ => {}
    }
}

/// Open a drive file full screen in the app for it (`app` overrides the choice by extension).
/// `name` is the file's display name (public links to one file have an empty path).
pub fn open_file(kind: SaveKind, path: String, name: String, app: Option<String>) {
    let ui = ui();
    wasm_bindgen_futures::spawn_local(async move {
        let mut name = name;
        if name.is_empty() {
            // A share of one file: its name comes from the share's listing.
            if let SaveKind::Share(id) = &kind {
                if let Ok(listing) = api::get_json::<artcraft_cloud_types::DirListing>(&format!(
                    "/api/shares/{id}/list?path={}",
                    api::enc(&path)
                ))
                .await
                {
                    name = listing
                        .entries
                        .first()
                        .map(|e| e.name.clone())
                        .unwrap_or_default();
                }
            }
        }
        let mut banner = String::new();
        let mut suggested = None;
        let mut chosen = app
            .clone()
            .or_else(|| app_for_name(&name).map(|app| app.id.to_string()));
        let is_zip = name.to_ascii_lowercase().ends_with(".zip");
        if is_zip
            && matches!(kind, SaveKind::Home)
            && chosen.as_deref().is_none_or(|id| id == "lightcraft")
        {
            if let Ok(found) = api::file_kind(&path).await {
                if found.app.as_deref() == Some("lightcraft") {
                    chosen = Some("lightcraft".into());
                }
            }
        }
        let Some(app_id) = chosen else {
            // Nothing opens it by its name: let the user pick an app (or download it).
            let share = match kind {
                SaveKind::Share(id) => Some(id),
                _ => None,
            };
            if ui.route.get_untracked().is_doc() {
                ui.replace(ui.route.get_untracked().home_of());
            }
            ui.open_modal(Modal::OpenWith { path, share });
            return;
        };
        let installed = ui
            .apps
            .with_untracked(|list| list.iter().any(|item| item.id == app_id && item.installed));
        let apps_known = ui.apps.with_untracked(|list| !list.is_empty());
        if apps_known && !installed {
            let title = app_by_id(&app_id).map(|a| a.title).unwrap_or(t("That app"));
            ui.say(format!(
                "{title} is not installed yet. It downloads in the background after startup."
            ));
            if ui.route.get_untracked().is_doc() {
                ui.replace(ui.route.get_untracked().home_of());
            }
            return;
        }
        // A LightCraft backup is restored through LightCraft's own menu, not opened as a photo.
        let restore = app_id == "lightcraft" && is_zip;
        let url = match &kind {
            SaveKind::Home => api::home_content(&path),
            SaveKind::Share(id) => api::share_content(*id, &path),
            SaveKind::Public(token) => api::public_content(token, &path),
        };
        let (bytes, etag) = if restore {
            banner = "This is a LightCraft library backup. In LightCraft choose File ▸ Restore Library from Backup…; the file picker offers this backup first.".into();
            suggested = Some(path.clone());
            (None, None)
        } else {
            match api::send("GET", &url, None, None, None).await {
                Ok(reply) if reply.status < 400 => (Some(reply.body), reply.etag),
                Ok(reply) => {
                    ui.say(String::from_utf8_lossy(&reply.body).to_string());
                    return;
                }
                Err(err) => {
                    ui.say(err);
                    return;
                }
            }
        };
        let display = if path.is_empty() {
            name.clone()
        } else {
            file_name(&path)
        };
        ui.editor.launch(Launch {
            app: app_id,
            name: display,
            path: if restore { None } else { Some(path.clone()) },
            etag,
            folder: parent_folder(&path),
            kind: Some(kind),
            read_only: false,
            bytes,
            banner,
            suggested,
        });
    });
}

fn download_href(route: &Route, path: &str) -> String {
    match route {
        Route::Files(_) => api::home_content(path),
        Route::Share(id, _) => api::share_content(*id, path),
        Route::Public(token) => api::public_content(token, path),
        _ => "#".into(),
    }
}

#[component]
fn ShareLists(incoming: RwSignal<Vec<Share>>, outgoing: RwSignal<Vec<Share>>) -> impl IntoView {
    let ui = ui();
    view! {
        <section class="card">
            <h2>{t("Shared with me")}</h2>
            <Show when=move || incoming.get().is_empty()><p class="empty">{t("Nothing has been shared with you yet.")}</p></Show>
            <ul class="list">
                <For each=move || incoming.get() key=|share| share.id let(share)>
                    <li>
                        <button type="button" class="file-name" on:click=move |_| {
                            if share.is_dir {
                                ui.go(Route::Share(share.id, String::new()));
                            } else {
                                ui.from_drive.set(true);
                                ui.go(Route::Open { share: Some(share.id), path: String::new(), app: app_of_name(&share.path).map(str::to_string) });
                            }
                        }>
                            <span class="file-icon">{if share.is_dir { icon(Icon::Folder).into_any() } else { match app_of_name(&share.path) { Some(app) => app_icon(app, "app-icon").into_any(), None => icon(Icon::File).into_any() } }}</span>
                            <span class="label">{file_name(&share.path)}</span>
                        </button>
                        <span class="muted">{format!("from {}{}", share.owner, share.group.as_deref().map(|g| format!(" · via {g}")).unwrap_or_default())}</span>
                        <span class="badge">{if share.permission == "edit" { t("Can edit") } else { t("Can view") }}</span>
                    </li>
                </For>
            </ul>
        </section>
        <section class="card">
            <h2>{t("Shared by me")}</h2>
            <Show when=move || outgoing.get().is_empty()><p class="empty">{t("You have not shared anything yet.")}</p></Show>
            <ul class="list">
                <For each=move || outgoing.get() key=|share| share.id let(share)>
                    <li>
                        <span class="file-icon">{if share.public { icon(Icon::Link) } else if share.group.is_some() { icon(Icon::People) } else { icon(Icon::Shared) }}</span>
                        <span class="label">{share.path.clone()}</span>
                        <span class="muted">{share_target(&share)}</span>
                        <span class="badge">{if share.permission == "edit" { t("Can edit") } else { t("Can view") }}</span>
                        <button type="button" class="button danger" on:click=move |_| {
                            let id = share.id;
                            wasm_bindgen_futures::spawn_local(async move {
                                if let Err(err) = api::delete(&format!("/api/shares/{id}")).await {
                                    ui.say(err);
                                }
                                ui.refresh();
                            });
                        }>{t("Stop sharing")}</button>
                    </li>
                </For>
            </ul>
        </section>
    }
}

pub fn share_target(share: &Share) -> String {
    if share.public {
        t("Anyone with the link").into()
    } else if let Some(group) = &share.group {
        tf("Group {group}", &[("group", group)])
    } else {
        share.grantee.clone().unwrap_or_default()
    }
}

#[component]
fn TrashList(rows: RwSignal<Vec<TrashEntry>>) -> impl IntoView {
    let ui = ui();
    view! {
        <section class="card">
            <div class="files-head">
                <h2>{t("Trash")}</h2>
                <Show when=move || !rows.get().is_empty()>
                    <button type="button" class="button danger" on:click=move |_| ui.open_modal(Modal::Purge { id: None })>{icon(Icon::Trash)}<span>{t("Empty trash")}</span></button>
                </Show>
            </div>
            <Show when=move || rows.get().is_empty()><p class="empty">{t("The trash is empty.")}</p></Show>
            <table class="file-table">
                <tbody>
                    <For each=move || rows.get() key=|row| row.id.clone() let(row)>
                        <tr class="row">
                            <td class="name-col"><span class="file-name static">
                                <span class="file-icon">{if row.is_dir { icon(Icon::Folder).into_any() } else { match app_of_name(&row.name) { Some(app) => app_icon(app, "app-icon").into_any(), None => icon(Icon::File).into_any() } }}</span>
                                <span class="label">{row.original_path.clone()}</span>
                            </span></td>
                            <td class="muted">{format!("Deleted {}", format_date(row.deleted_at))}</td>
                            <td class="row-actions">
                                <button type="button" class="icon-button" title=t("Restore") aria-label=t("Restore") on:click={
                                    let id = row.id.clone();
                                    move |_| {
                                        let id = id.clone();
                                        wasm_bindgen_futures::spawn_local(async move {
                                            if let Err(err) = api::post_empty(&format!("/api/trash/{id}/restore")).await {
                                                ui.say(err);
                                            }
                                            ui.refresh();
                                        });
                                    }
                                }>{icon(Icon::Restore)}</button>
                                <button type="button" class="icon-button danger" title=t("Delete forever") aria-label=t("Delete forever") on:click={
                                    let id = row.id.clone();
                                    move |_| ui.open_modal(Modal::Purge { id: Some(id.clone()) })
                                }>{icon(Icon::Trash)}</button>
                            </td>
                        </tr>
                    </For>
                </tbody>
            </table>
        </section>
    }
}

#[component]
fn PublicUnlock() -> impl IntoView {
    let ui = ui();
    let password = RwSignal::new(String::new());
    view! {
        <form class="card unlock" on:submit=move |ev| {
            ev.prevent_default();
            let Route::Public(token) = ui.route.get_untracked() else { return };
            let password = password.get_untracked();
            wasm_bindgen_futures::spawn_local(async move {
                match api::post_json::<_, serde_json::Value>(&format!("/api/public/{token}/unlock"), &serde_json::json!({"password": password})).await {
                    Ok(_) => ui.refresh(),
                    Err(err) => ui.say(err),
                }
            });
        }>
            <h2>{t("This link has a password")}</h2>
            <input class="field" type="password" placeholder=t("Password") prop:value=move || password.get() on:input=move |ev| password.set(event_target_value(&ev)) />
            <button type="submit" class="button primary">{t("Unlock")}</button>
        </form>
    }
}

pub fn format_size(size: u64) -> String {
    if size < 1024 {
        format!("{size} B")
    } else if size < 1024 * 1024 {
        format!("{} KB", size / 1024)
    } else {
        format!("{:.1} MB", size as f64 / 1024.0 / 1024.0)
    }
}

/// A Unix time as a short local date ("23 Nov 2026").
pub fn format_date(secs: i64) -> String {
    if secs <= 0 {
        return String::new();
    }
    #[cfg(target_arch = "wasm32")]
    {
        let date = js_sys::Date::new(&wasm_bindgen::JsValue::from_f64(secs as f64 * 1000.0));
        let options = js_sys::Object::new();
        let _ = js_sys::Reflect::set(&options, &"day".into(), &"numeric".into());
        let _ = js_sys::Reflect::set(&options, &"month".into(), &"short".into());
        let _ = js_sys::Reflect::set(&options, &"year".into(), &"numeric".into());
        String::from(date.to_locale_date_string("default", &options))
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        secs.to_string()
    }
}

pub fn event_target<T: JsCast>(ev: &web_sys::Event) -> T {
    ev.target()
        .and_then(|target| target.dyn_into().ok())
        .expect("event target")
}

/// The apps that can open a file, best first, with whether each is installed.
pub fn apps_for(name: &str, apps: &[AppStatus]) -> Vec<(String, String, bool, bool)> {
    let best = app_of_name(name);
    let mut out: Vec<(String, String, bool, bool)> = catalog()
        .iter()
        .filter(|app| app.bridge)
        .map(|app| {
            let installed = apps.iter().any(|a| a.id == app.id && a.installed);
            (
                app.id.to_string(),
                app.title.to_string(),
                installed,
                Some(app.id) == best,
            )
        })
        .collect();
    out.sort_by_key(|(_, _, installed, best)| (!*best, !*installed));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app(id: &str, title: &str, extensions: &[&str]) -> AppStatus {
        AppStatus {
            id: id.into(),
            title: title.into(),
            extensions: extensions.iter().map(|e| e.to_string()).collect(),
            bridge: true,
            ..AppStatus::default()
        }
    }

    #[test]
    fn files_map_to_app_icons() {
        crate::catalog::set(&[
            app("wordcraft", "WordCraft", &["docx", "txt"]),
            app("gridcraft", "GridCraft", &["xlsx"]),
            app("pdfcraft", "PdfCraft", &["pdf"]),
            app("lightcraft", "LightCraft", &["jpg"]),
        ]);
        assert_eq!(app_of_name("Hola.docx"), Some("wordcraft"));
        assert_eq!(app_of_name("Book1.XLSX"), Some("gridcraft"));
        assert_eq!(
            app_of_name("lightcraft-library-backup-2026-11-23.zip"),
            Some("lightcraft")
        );
        assert_eq!(app_of_name("archive.zip"), None);
        let ordered = apps_for("x.pdf", &[]);
        assert_eq!(ordered[0].0, "pdfcraft");
        assert!(ordered[0].3);
    }
}
