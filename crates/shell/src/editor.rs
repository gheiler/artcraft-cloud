//! Parent side of the `craft-web-host` contract (`?host=parent`), and the full-screen document view.

use crate::i18n::{t, tf};
use std::rc::Rc;

use crate::catalog::{app_by_id, app_for_name};
use artcraft_cloud_types::{conflict_copy_name, HOST_CONTRACT_VERSION};
use leptos::prelude::*;
use wasm_bindgen::{closure::Closure, JsCast, JsValue};
use web_sys::{HtmlIFrameElement, MessageEvent};

use crate::api::{self, PutFail};
use crate::icons::{app_icon, icon, Icon};
use crate::picker;
use crate::route::Route;
use crate::settings;
use crate::ui::{ui, Modal};

#[derive(Clone, Debug, PartialEq)]
pub enum SaveKind {
    Home,
    Share(i64),
    Public(String),
}

#[derive(Copy, Clone)]
pub struct Editor {
    pub open: RwSignal<bool>,
    pub app: RwSignal<String>,
    pub title: RwSignal<String>,
    pub name: RwSignal<String>,
    pub path: RwSignal<Option<String>>,
    pub etag: RwSignal<Option<String>>,
    pub folder: RwSignal<String>,
    pub kind: RwSignal<SaveKind>,
    pub read_only: RwSignal<bool>,
    pub dirty: RwSignal<bool>,
    pub status: RwSignal<String>,
    pub bytes: RwSignal<Option<Vec<u8>>>,
    pub sent: RwSignal<bool>,
    pub author: RwSignal<String>,
    /// The restored app options name an author: `?author=` is left out so they win.
    pub own_author: RwSignal<bool>,
    pub conflict: RwSignal<Option<Vec<u8>>>,
    pub save_as: RwSignal<Option<(String, Vec<u8>, bool)>>,
    /// A hint shown over the app (for example how to restore a LightCraft backup).
    pub banner: RwSignal<String>,
    /// A drive file the app's next file picker offers first.
    pub suggested: RwSignal<Option<String>>,
    pub capability: RwSignal<Option<String>>,
    pub app_settings: RwSignal<serde_json::Map<String, serde_json::Value>>,
    pub autosave_off: RwSignal<Vec<String>>,
    /// When the document became dirty, and when the shell last asked the app to save (ms).
    pub dirty_since: StoredValue<f64>,
    pub last_try: StoredValue<f64>,
    pub frame: StoredValue<Option<HtmlIFrameElement>, LocalStorage>,
}

/// Everything needed to put a document on screen.
#[derive(Clone, Debug, Default)]
pub struct Launch {
    pub app: String,
    pub name: String,
    pub path: Option<String>,
    pub etag: Option<String>,
    pub folder: String,
    pub kind: Option<SaveKind>,
    pub read_only: bool,
    pub bytes: Option<Vec<u8>>,
    pub banner: String,
    pub suggested: Option<String>,
}

/// Ask the app to save this long after it became dirty.
const AUTOSAVE_AFTER_MS: f64 = 2500.0;
/// And at most this often while it stays dirty.
const AUTOSAVE_EVERY_MS: f64 = 8000.0;
const AUTOSAVE_KEY: &str = "autosave_off";

std::thread_local! {
    static PENDING_LAUNCH: std::cell::RefCell<Option<Launch>> = const { std::cell::RefCell::new(None) };
}

/// Continue opening after the person accepted the unverified-app warning.
pub fn take_pending_launch() -> Option<Launch> {
    PENDING_LAUNCH.with(|slot| slot.borrow_mut().take())
}

impl Editor {
    pub fn new() -> Self {
        Self {
            open: RwSignal::new(false),
            app: RwSignal::new(String::new()),
            title: RwSignal::new(String::new()),
            name: RwSignal::new(String::new()),
            path: RwSignal::new(None),
            etag: RwSignal::new(None),
            folder: RwSignal::new(String::new()),
            kind: RwSignal::new(SaveKind::Home),
            read_only: RwSignal::new(false),
            dirty: RwSignal::new(false),
            status: RwSignal::new(String::new()),
            bytes: RwSignal::new(None),
            sent: RwSignal::new(false),
            author: RwSignal::new(String::new()),
            own_author: RwSignal::new(false),
            conflict: RwSignal::new(None),
            save_as: RwSignal::new(None),
            banner: RwSignal::new(String::new()),
            suggested: RwSignal::new(None),
            capability: RwSignal::new(None),
            app_settings: RwSignal::new(serde_json::Map::new()),
            autosave_off: RwSignal::new(Vec::new()),
            dirty_since: StoredValue::new(0.0),
            last_try: StoredValue::new(0.0),
            frame: StoredValue::new_local(None),
        }
    }

    /// Restore the user's options for the app, then start it with the document.
    pub fn launch(&self, spec: Launch) {
        let needs = crate::ui::ui().apps.with_untracked(|list| {
            list.iter()
                .any(|a| a.id == spec.app && a.unverified && a.needs_ack)
        });
        if needs {
            PENDING_LAUNCH.with(|slot| *slot.borrow_mut() = Some(spec.clone()));
            let title = crate::ui::ui()
                .apps
                .with_untracked(|list| {
                    list.iter()
                        .find(|a| a.id == spec.app)
                        .map(|a| a.title.clone())
                })
                .unwrap_or_else(|| spec.app.clone());
            crate::ui::ui().open_modal(crate::ui::Modal::Unverified {
                app: spec.app.clone(),
                title,
            });
            return;
        }
        self.launch_now(spec);
    }

    pub fn launch_now(&self, spec: Launch) {
        let editor = *self;
        let user = ui()
            .user
            .with_untracked(|user| user.as_ref().map(|u| u.id))
            .unwrap_or(0);
        wasm_bindgen_futures::spawn_local(async move {
            if editor.open.get_untracked() {
                settings::leave();
                let _ = editor.open.try_set(false);
            }
            let mode = isolation_of(&spec.app);
            let unverified = app_by_id(&spec.app).is_some_and(|item| item.unverified);
            let own_author = if mode == "sandbox" {
                let saved = crate::api::settings(&spec.app).await.unwrap_or_default();
                let _ = editor.app_settings.try_set(saved);
                false
            } else if mode == "domain" {
                let _ = editor.app_settings.try_set(serde_json::Map::new());
                false
            } else {
                settings::restore(&spec.app, user).await
            };
            if unverified {
                let body = if let Some(path) = spec.path.as_ref().filter(|p| !p.is_empty()) {
                    serde_json::json!({ "path": path })
                } else {
                    serde_json::json!({ "create": true })
                };
                match crate::api::json::<serde_json::Value>(
                    "POST",
                    &format!("/api/apps/{}/capability", spec.app),
                    &body,
                )
                .await
                {
                    Ok(cap) => {
                        let token = cap
                            .get("token")
                            .and_then(|v| v.as_str())
                            .map(str::to_string);
                        let _ = editor.capability.try_set(token);
                    }
                    Err(err) => {
                        let _ = editor.capability.try_set(None);
                        web_sys::console::warn_1(
                            &format!("artcraftCloud: could not issue a capability token: {err}")
                                .into(),
                        );
                    }
                }
            } else {
                let _ = editor.capability.try_set(None);
                let _ = editor.app_settings.try_set(serde_json::Map::new());
            }
            let title = app_by_id(&spec.app)
                .map(|app| app.title)
                .unwrap_or(t("App"))
                .to_string();
            let _ = editor.own_author.try_set(own_author);
            let _ = editor.app.try_set(spec.app);
            let _ = editor.title.try_set(title);
            let _ = editor.name.try_set(spec.name);
            let _ = editor.path.try_set(spec.path);
            let _ = editor.etag.try_set(spec.etag);
            let _ = editor.folder.try_set(spec.folder);
            let _ = editor.kind.try_set(spec.kind.unwrap_or(SaveKind::Home));
            let _ = editor.read_only.try_set(spec.read_only);
            let _ = editor.dirty.try_set(false);
            let _ = editor.status.try_set(if spec.bytes.is_some() {
                t("Opening…").into()
            } else {
                String::new()
            });
            let _ = editor.bytes.try_set(spec.bytes);
            let _ = editor.sent.try_set(false);
            let _ = editor.conflict.try_set(None);
            let _ = editor.save_as.try_set(None);
            let _ = editor.banner.try_set(spec.banner);
            let _ = editor.suggested.try_set(spec.suggested);
            let _ = editor.open.try_set(true);
            set_title(editor);
        });
    }

    /// Close without asking. Callers ask first when the document is dirty (see [`leave_doc`]).
    pub fn close_now(&self) {
        settings::leave();
        let _ = self.dirty.try_set(false);
        let _ = self.open.try_set(false);
        let _ = self.bytes.try_set(None);
        let _ = self.conflict.try_set(None);
        let _ = self.save_as.try_set(None);
        let _ = self.banner.try_set(String::new());
        let _ = self.suggested.try_set(None);
        let _ = self.status.try_set(String::new());
        if let Some(document) = web_sys::window().and_then(|w| w.document()) {
            document.set_title("artcraftCloud");
        }
    }

    pub fn autosave_on(&self) -> bool {
        let app = self.app.get();
        ui().prefs.with(|p| p.autosave) && !self.autosave_off.with(|off| off.contains(&app))
    }

    pub fn toggle_autosave(&self) {
        let app = self.app.get_untracked();
        // Turning it on for one app while Settings has it off everywhere: on here, off elsewhere.
        if !ui().prefs.with_untracked(|p| p.autosave) {
            let others: Vec<String> = crate::catalog::catalog()
                .iter()
                .map(|a| a.id.to_string())
                .filter(|id| *id != app)
                .collect();
            self.set_autosave_off(others);
            let mut next = ui().prefs.get_untracked();
            next.autosave = true;
            crate::preferences::save(next);
            return;
        }
        self.autosave_off.update(|off| {
            if let Some(index) = off.iter().position(|id| *id == app) {
                off.remove(index);
            } else {
                off.push(app);
            }
        });
        let off = self.autosave_off.get_untracked();
        wasm_bindgen_futures::spawn_local(async move {
            let mut shell = api::settings("shell").await.unwrap_or_default();
            shell.insert(AUTOSAVE_KEY.into(), serde_json::json!(off));
            let _ = api::put_settings("shell", &shell).await;
        });
    }

    /// Store which apps have autosave off.
    pub fn set_autosave_off(&self, off: Vec<String>) {
        let _ = self.autosave_off.try_set(off.clone());
        wasm_bindgen_futures::spawn_local(async move {
            let mut shell = api::settings("shell").await.unwrap_or_default();
            shell.insert(AUTOSAVE_KEY.into(), serde_json::json!(off));
            let _ = api::put_settings("shell", &shell).await;
        });
    }

    /// Load the shell's own options (which apps the user turned autosave off for).
    pub fn load_shell_settings(&self) {
        let editor = *self;
        wasm_bindgen_futures::spawn_local(async move {
            if let Ok(shell) = api::settings("shell").await {
                let off = shell
                    .get(AUTOSAVE_KEY)
                    .and_then(|v| v.as_array())
                    .map(|list| {
                        list.iter()
                            .filter_map(|v| v.as_str().map(str::to_string))
                            .collect()
                    })
                    .unwrap_or_default();
                let _ = editor.autosave_off.try_set(off);
            }
        });
    }
}

/// Leave the open document for `then` (the drive when `None`), asking first when it has unsaved
/// changes.
pub fn leave_doc(then: Option<Route>) {
    let ui = ui();
    if ui.editor.dirty.get_untracked() {
        ui.open_modal(Modal::Discard { then });
        return;
    }
    finish_leave(then);
}

pub fn finish_leave(then: Option<Route>) {
    let ui = ui();
    let current = ui.route.get_untracked();
    ui.editor.close_now();
    match then {
        // One document to another (File ▸ Open): replace, so Back still leads to the drive.
        Some(route) if route.is_doc() && current.is_doc() => ui.replace(route),
        Some(route) => ui.go(route),
        None if ui.from_drive.get_untracked() && current.is_doc() => {
            ui.from_drive.set(false);
            if let Some(history) = web_sys::window().and_then(|w| w.history().ok()) {
                let _ = history.back();
            }
        }
        None => ui.go(current.home_of()),
    }
}

fn set_title(editor: Editor) {
    if let Some(document) = web_sys::window().and_then(|window| window.document()) {
        let mark = if editor.dirty.get_untracked() {
            "● "
        } else {
            ""
        };
        document.set_title(&format!(
            "{mark}{} — artcraftCloud",
            editor.name.get_untracked()
        ));
    }
}

pub fn install(editor: Editor) {
    let window = match web_sys::window() {
        Some(window) => window,
        None => return,
    };
    let origin = window.location().origin().unwrap_or_default();
    let on_message = Closure::wrap(Box::new(move |event: MessageEvent| {
        let ev_origin = event.origin();
        let app = editor.app.get_untracked();
        let allowed = ev_origin == origin
            || (is_sandboxed(&app) && ev_origin == "null")
            || (uses_apps_host(&app) && ev_origin == frame_origin(&app));
        if !allowed {
            return;
        }
        let Some(frame) = current_frame(editor) else {
            return;
        };
        let Some(source) = event.source() else {
            return;
        };
        let Some(content) = frame.content_window() else {
            return;
        };
        if !js_sys::Object::is(&source, &content) {
            return;
        }
        let data = event.data();
        let Some(kind) = message_kind(&data, &editor.app.get_untracked()) else {
            return;
        };
        match kind.as_str() {
            "ready" => on_ready(editor, &frame),
            "dirty" => {
                let dirty = bool_field(&data, "dirty").unwrap_or(false);
                if dirty && !editor.dirty.get_untracked() {
                    editor.dirty_since.set_value(now_ms());
                }
                let _ = editor.dirty.try_set(dirty);
            }
            "save" => on_save(editor, &data),
            "pick" => on_pick(editor, &data),
            "failed" => {
                let _ = editor.status.try_set(
                    string_field(&data, "error")
                        .unwrap_or_else(|| "the app could not open the file".into()),
                );
            }
            "handoff" => on_handoff(editor, &data, &frame),
            "settings-set" => on_settings_set(editor, &data),
            "settings-remove" => on_settings_remove(editor, &data),
            _ => {}
        }
        set_title(editor);
    }) as Box<dyn FnMut(MessageEvent)>);
    let _ = window.add_event_listener_with_callback("message", on_message.as_ref().unchecked_ref());
    on_message.forget();

    let on_leave = Closure::wrap(Box::new(move |event: web_sys::BeforeUnloadEvent| {
        if editor.dirty.get_untracked() {
            event.prevent_default();
            event.set_return_value("unsaved");
        }
    }) as Box<dyn FnMut(web_sys::BeforeUnloadEvent)>);
    let _ =
        window.add_event_listener_with_callback("beforeunload", on_leave.as_ref().unchecked_ref());
    on_leave.forget();

    let tick = Closure::<dyn FnMut()>::new(move || autosave_tick(editor));
    let _ = window.set_interval_with_callback_and_timeout_and_arguments_0(
        tick.as_ref().unchecked_ref(),
        1000,
    );
    tick.forget();

    picker::set_isolated_reply(move |request_id, files| {
        reply_isolated(editor, request_id, files);
    });
}

fn now_ms() -> f64 {
    js_sys::Date::now()
}

/// Autosave from the host: the apps' own AutoSave writes through a desktop file path that the web
/// build does not have (GridCraft's switch saves nothing in a browser). The shell instead asks the
/// app to run its own `file.save` command through its page API, which posts `<app>:save` like
/// Ctrl+S does, and the save goes to the drive with the usual conflict check.
fn autosave_tick(editor: Editor) {
    let Some(open) = editor.open.try_get_untracked() else {
        return;
    };
    if !open || !editor.dirty.get_untracked() || editor.read_only.get_untracked() {
        return;
    }
    if editor.path.get_untracked().is_none()
        || editor.conflict.with_untracked(Option::is_some)
        || editor.save_as.with_untracked(Option::is_some)
    {
        return;
    }
    let app = editor.app.get_untracked();
    if !ui().prefs.with_untracked(|p| p.autosave)
        || editor.autosave_off.with_untracked(|off| off.contains(&app))
    {
        return;
    }
    let now = now_ms();
    if now - editor.dirty_since.get_value() < AUTOSAVE_AFTER_MS
        || now - editor.last_try.get_value() < AUTOSAVE_EVERY_MS
    {
        return;
    }
    editor.last_try.set_value(now);
    if run_command(editor, "file.save") {
        let _ = editor.status.try_set(t("Autosaving…").into());
    }
}

/// Run one of the app's commands through its page API (`window.<app>.execute`). Returns false
/// when the app has no page API.
pub fn run_command(editor: Editor, command: &str) -> bool {
    let Some(frame) = current_frame(editor) else {
        return false;
    };
    let Some(window) = frame.content_window() else {
        return false;
    };
    let app = editor.app.get_untracked();
    let Ok(api) = js_sys::Reflect::get(&window, &JsValue::from_str(&app)) else {
        return false;
    };
    let Ok(execute) = js_sys::Reflect::get(&api, &JsValue::from_str("execute")) else {
        return false;
    };
    let Some(execute) = execute.dyn_ref::<js_sys::Function>() else {
        return false;
    };
    execute
        .call2(&api, &JsValue::from_str(command), &js_sys::Object::new())
        .is_ok()
}

/// The live editor iframe. `<Show>` builds a new iframe for every file opened, and the node ref
/// only reports the first one, so look it up in the document and keep the ref as a fallback.
fn current_frame(editor: Editor) -> Option<HtmlIFrameElement> {
    let live = web_sys::window()
        .and_then(|window| window.document())
        .and_then(|document| {
            document
                .query_selector("section.editor iframe")
                .ok()
                .flatten()
        })
        .and_then(|element| element.dyn_into::<HtmlIFrameElement>().ok());
    live.or_else(|| editor.frame.try_with_value(|frame| frame.clone()).flatten())
}

fn on_ready(editor: Editor, frame: &HtmlIFrameElement) {
    if editor.sent.get_untracked() {
        return;
    }
    let app = editor.app.get_untracked();
    let mut host_fields = vec![
        ("version", JsValue::from_str(HOST_CONTRACT_VERSION)),
        ("handoff", JsValue::TRUE),
    ];
    if let Some(token) = editor.capability.get_untracked() {
        host_fields.push(("token", JsValue::from_str(&token)));
    }
    let settings = editor.app_settings.get_untracked();
    if !settings.is_empty() {
        if let Ok(value) =
            js_sys::JSON::parse(&serde_json::to_string(&settings).unwrap_or_else(|_| "{}".into()))
        {
            host_fields.push(("settings", value));
        }
    }
    post(frame, &app, "host", &host_fields, None);
    if let Some(bytes) = editor.bytes.try_update(|slot| slot.take()).flatten() {
        let name = editor.name.get_untracked();
        let buffer = bytes_to_buffer(&bytes);
        post(
            frame,
            &editor.app.get_untracked(),
            "open",
            &[
                ("name", JsValue::from_str(&name)),
                ("bytes", buffer.clone().into()),
            ],
            Some(&buffer),
        );
        let _ = editor.status.try_set(String::new());
    }
    let _ = editor.sent.try_set(true);
}

fn on_save(editor: Editor, data: &JsValue) {
    let name = string_field(data, "name").unwrap_or_else(|| editor.name.get_untracked());
    let save_as = bool_field(data, "saveAs").unwrap_or(false);
    let Some(bytes) = bytes_field(data) else {
        let _ = editor
            .status
            .try_set(t("The app sent a save with no bytes.").into());
        return;
    };
    if !has_grant(&editor.app.get_untracked(), "document") {
        let _ = editor
            .status
            .try_set(t("This app is not allowed to save files.").into());
        return;
    }
    if editor.read_only.get_untracked() {
        let _ = editor
            .status
            .try_set(t("View only — the file was not stored.").into());
        return;
    }
    let current = editor.path.get_untracked();
    // Compare with the shown name: a share of one file has an empty path inside the share.
    let same_file = current.is_some() && editor.name.get_untracked() == name;
    if current.is_some() && !save_as && same_file {
        let bytes = Rc::new(bytes);
        wasm_bindgen_futures::spawn_local(async move {
            overwrite(editor, bytes).await;
        });
    } else if save_as || current.is_none() {
        let _ = editor.save_as.try_set(Some((name, bytes, true)));
        ui().open_modal(Modal::SaveAs);
    } else {
        let bytes = Rc::new(bytes);
        let target_name = name;
        wasm_bindgen_futures::spawn_local(async move {
            write_new(editor, &target_name, bytes, false).await;
        });
    }
}

fn on_handoff(editor: Editor, data: &JsValue, frame: &HtmlIFrameElement) {
    let name = string_field(data, "name").unwrap_or_else(|| "document".into());
    let Some(bytes) = bytes_field(data) else {
        return;
    };
    let Some(app) = app_for_name(&name) else {
        let _ = editor
            .status
            .try_set(tf("No installed app opens {name}.", &[("name", &name)]));
        return;
    };
    post(
        frame,
        &editor.app.get_untracked(),
        "handoff-done",
        &[
            ("name", JsValue::from_str(&name)),
            ("app", JsValue::from_str(app.id)),
        ],
        None,
    );
    let bytes = Rc::new(bytes);
    let app_id = app.id.to_string();
    let folder = editor.folder.get_untracked();
    let kind = editor.kind.get_untracked();
    wasm_bindgen_futures::spawn_local(async move {
        if let Some(full) = write_new(editor, &name, Rc::clone(&bytes), false).await {
            let share = match kind {
                SaveKind::Share(id) => Some(id),
                _ => None,
            };
            if !matches!(kind, SaveKind::Public(_)) {
                ui().replace(Route::Open {
                    share,
                    path: full.clone(),
                    app: None,
                });
            }
            editor.launch(Launch {
                app: app_id,
                name: file_name(&full),
                path: Some(full),
                folder,
                kind: Some(kind),
                bytes: Some((*bytes).clone()),
                banner: t("Handed off from the previous app.").into(),
                ..Launch::default()
            });
        }
    });
}

async fn overwrite(editor: Editor, bytes: Rc<Vec<u8>>) -> Option<()> {
    let path = editor.path.get_untracked()?;
    let url = content_url(editor, &relative_of(editor, &path));
    let if_match = editor
        .etag
        .get_untracked()
        .map(|etag| format!("\"{etag}\""));
    match api::put_bytes(&url, &bytes, if_match.as_deref()).await {
        Ok(written) => {
            let _ = editor.etag.try_set(Some(written.etag));
            let _ = editor.dirty.try_set(false);
            let _ = editor.status.try_set(t("Saved.").into());
            Some(())
        }
        Err(PutFail::Conflict(etag)) => {
            if let Some(etag) = etag {
                let _ = editor.etag.try_set(Some(etag));
            }
            let _ = editor.conflict.try_set(Some((*bytes).clone()));
            let _ = editor.status.try_set(
                t("Someone else changed this file. Save a copy to keep your version.").into(),
            );
            None
        }
        Err(PutFail::Other(err)) => {
            let _ = editor.status.try_set(err);
            None
        }
    }
}

async fn write_new(editor: Editor, name: &str, bytes: Rc<Vec<u8>>, rebind: bool) -> Option<String> {
    for n in 0..20 {
        let candidate = if n == 0 {
            name.to_string()
        } else {
            conflict_copy_name(name, n)
        };
        let full = join_folder(&editor.folder.get_untracked(), &candidate);
        let url = content_url(editor, &relative_of(editor, &full));
        match api::put_bytes(&url, &bytes, None).await {
            Ok(written) => {
                let _ = editor
                    .status
                    .try_set(tf("Saved {candidate}.", &[("candidate", &candidate)]));
                if rebind || editor.path.get_untracked().is_none() {
                    let _ = editor.path.try_set(Some(full.clone()));
                    let _ = editor.name.try_set(candidate);
                    let _ = editor.etag.try_set(Some(written.etag));
                    let _ = editor.dirty.try_set(false);
                    // The document now has an address of its own.
                    let share = match editor.kind.get_untracked() {
                        SaveKind::Share(id) => Some(id),
                        _ => None,
                    };
                    if !matches!(editor.kind.get_untracked(), SaveKind::Public(_)) {
                        ui().replace(Route::Open {
                            share,
                            path: full.clone(),
                            app: None,
                        });
                    }
                    set_title(editor);
                }
                return Some(full);
            }
            Err(PutFail::Conflict(_)) if n == 0 => continue,
            Err(PutFail::Conflict(_)) => continue,
            Err(PutFail::Other(err)) => {
                let _ = editor.status.try_set(err);
                return None;
            }
        }
    }
    let _ = editor
        .status
        .try_set(t("Could not find a free name.").into());
    None
}

pub fn content_url(editor: Editor, relative: &str) -> String {
    match editor.kind.get_untracked() {
        SaveKind::Home => api::home_content(relative),
        SaveKind::Share(id) => api::share_content(id, relative),
        SaveKind::Public(token) => api::public_content(&token, relative),
    }
}

fn relative_of(editor: Editor, full: &str) -> String {
    match editor.kind.get_untracked() {
        SaveKind::Home => full.to_string(),
        SaveKind::Share(_) | SaveKind::Public(_) => full.to_string(),
    }
}

pub fn join_folder(folder: &str, name: &str) -> String {
    if folder.is_empty() {
        name.to_string()
    } else {
        format!("{folder}/{name}")
    }
}

pub fn file_name(path: &str) -> String {
    path.rsplit('/').next().unwrap_or(path).to_string()
}

pub fn parent_folder(path: &str) -> String {
    match path.rfind('/') {
        Some(index) => path[..index].to_string(),
        None => String::new(),
    }
}

fn apps_origin() -> Option<String> {
    ui().user
        .with_untracked(|user| user.as_ref().and_then(|u| u.apps_origin.clone()))
}

fn isolation_of(app: &str) -> String {
    let stored = app_by_id(app)
        .map(|item| item.isolation.trim().to_string())
        .filter(|s| !s.is_empty());
    if let Some(mode) = stored {
        return mode;
    }
    if app_by_id(app).is_some_and(|item| item.isolated) {
        "sandbox".into()
    } else {
        String::new()
    }
}

fn is_sandboxed(app: &str) -> bool {
    isolation_of(app) == "sandbox"
}

fn uses_apps_host(app: &str) -> bool {
    isolation_of(app) == "domain"
}

fn is_cross_origin(app: &str) -> bool {
    let mode = isolation_of(app);
    mode == "sandbox" || mode == "domain"
}

fn frame_origin(app: &str) -> String {
    if uses_apps_host(app) {
        if let Some(origin) = apps_origin() {
            return origin;
        }
    }
    web_sys::window()
        .and_then(|w| w.location().origin().ok())
        .unwrap_or_default()
}

fn grants_for(app: &str) -> Vec<String> {
    let grants = ui().apps.with_untracked(|list| {
        list.iter()
            .find(|item| item.id == app)
            .map(|item| item.grants.clone())
            .unwrap_or_default()
    });
    if grants.is_empty() && !app_by_id(app).is_some_and(|item| item.unverified) {
        vec![
            "document".into(),
            "drive".into(),
            "clipboard".into(),
            "fullscreen".into(),
        ]
    } else {
        grants
    }
}

fn has_grant(app: &str, grant: &str) -> bool {
    grants_for(app).iter().any(|item| item == grant)
}

fn u32_field(data: &JsValue, key: &str) -> Option<u32> {
    js_sys::Reflect::get(data, &JsValue::from_str(key))
        .ok()?
        .as_f64()
        .map(|n| n as u32)
}

fn on_pick(editor: Editor, data: &JsValue) {
    let app = editor.app.get_untracked();
    let request_id = u32_field(data, "id").unwrap_or(0);
    if !has_grant(&app, "drive") {
        reply_isolated(editor, request_id, None);
        let _ = editor
            .status
            .try_set(t("This app is not allowed to open other files.").into());
        return;
    }
    let accept = string_field(data, "accept").unwrap_or_default();
    let multiple = bool_field(data, "multiple").unwrap_or(false);
    let id = picker::remember_isolated(request_id);
    ui().open_modal(Modal::Pick {
        id,
        accept,
        multiple,
    });
}

fn reply_isolated(editor: Editor, request_id: u32, files: Option<Vec<(String, Vec<u8>)>>) {
    let Some(frame) = current_frame(editor) else {
        return;
    };
    let app = editor.app.get_untracked();
    let Some(window) = frame.content_window() else {
        return;
    };
    let origin = if is_sandboxed(&app) {
        "*".to_string()
    } else {
        frame_origin(&app)
    };
    let message = js_sys::Object::new();
    let kind = if files.is_some() {
        "picked"
    } else {
        "pick-cancel"
    };
    let _ = js_sys::Reflect::set(
        &message,
        &JsValue::from_str("type"),
        &JsValue::from_str(&format!("{app}:{kind}")),
    );
    let _ = js_sys::Reflect::set(
        &message,
        &JsValue::from_str("id"),
        &JsValue::from_f64(request_id as f64),
    );
    if let Some(files) = files {
        let list = js_sys::Array::new();
        let transfer = js_sys::Array::new();
        for (name, bytes) in files {
            let file = js_sys::Object::new();
            let buffer = bytes_to_buffer(&bytes);
            let _ =
                js_sys::Reflect::set(&file, &JsValue::from_str("name"), &JsValue::from_str(&name));
            let _ = js_sys::Reflect::set(
                &file,
                &JsValue::from_str("type"),
                &JsValue::from_str(picker::mime_of(&name)),
            );
            let _ = js_sys::Reflect::set(&file, &JsValue::from_str("bytes"), &buffer);
            list.push(&file);
            transfer.push(&buffer);
        }
        let _ = js_sys::Reflect::set(&message, &JsValue::from_str("files"), &list);
        let _ = window.post_message_with_transfer(&message, &origin, &transfer);
    } else {
        let _ = window.post_message(&message, &origin);
    }
}

fn on_settings_set(editor: Editor, data: &JsValue) {
    let Some(key) = string_field(data, "key") else {
        return;
    };
    let value = string_field(data, "value").unwrap_or_default();
    editor.app_settings.update(|map| {
        map.insert(key, serde_json::Value::String(value));
    });
    flush_app_settings(editor);
}

fn on_settings_remove(editor: Editor, data: &JsValue) {
    let Some(key) = string_field(data, "key") else {
        return;
    };
    editor.app_settings.update(|map| {
        map.remove(&key);
    });
    flush_app_settings(editor);
}

fn flush_app_settings(editor: Editor) {
    let app = editor.app.get_untracked();
    let map = editor.app_settings.get_untracked();
    wasm_bindgen_futures::spawn_local(async move {
        if let Err(err) = crate::api::put_settings(&app, &map).await {
            web_sys::console::warn_1(
                &format!("artcraftCloud: could not save {app} options: {err}").into(),
            );
        }
    });
}

pub fn iframe_allow(app: &str) -> String {
    let grants = ui().apps.with_untracked(|list| {
        list.iter()
            .find(|a| a.id == app)
            .map(|a| a.grants.clone())
            .unwrap_or_default()
    });
    let grants = if grants.is_empty() && !app_by_id(app).is_some_and(|a| a.unverified) {
        vec!["clipboard".into(), "fullscreen".into()]
    } else {
        grants
    };
    let mut parts = Vec::new();
    if grants.iter().any(|g| g == "fullscreen") {
        parts.push("fullscreen");
    }
    if grants.iter().any(|g| g == "clipboard") {
        parts.push("clipboard-read");
        parts.push("clipboard-write");
    }
    parts.join("; ")
}

pub fn iframe_src(app: &str, author: &str, own_author: bool) -> String {
    let path = if app_by_id(app).is_some_and(|item| !item.bridge) {
        // No host bridge (MailCraft): the app runs on its own, with nothing handed over.
        format!("/apps/{app}/index.html")
    } else {
        let mut url = format!("/apps/{app}/index.html?host=parent");
        if !own_author
            && !author.trim().is_empty()
            && app_by_id(app).is_some_and(|item| item.author_query)
        {
            url.push_str("&author=");
            url.push_str(&api::enc(author.trim()));
        }
        if app_by_id(app).is_some_and(|item| item.chrome_query) {
            url.push_str("&chrome=nofiletab");
        }
        url
    };
    if uses_apps_host(app) {
        if let Some(origin) = apps_origin() {
            return format!("{origin}{path}");
        }
    }
    path
}

fn post(
    frame: &HtmlIFrameElement,
    app: &str,
    kind: &str,
    fields: &[(&str, JsValue)],
    transfer: Option<&js_sys::ArrayBuffer>,
) {
    let Some(window) = frame.content_window() else {
        return;
    };
    // Sandboxed iframes (no allow-same-origin) have an opaque origin; the only legal
    // targetOrigin is "*". The message still goes only to this iframe's window.
    let origin = if is_sandboxed(app) {
        "*".to_string()
    } else {
        frame_origin(app)
    };
    if origin.is_empty() {
        return;
    }
    let message = js_sys::Object::new();
    let _ = js_sys::Reflect::set(
        &message,
        &JsValue::from_str("type"),
        &JsValue::from_str(&format!("{app}:{kind}")),
    );
    for (key, value) in fields {
        let _ = js_sys::Reflect::set(&message, &JsValue::from_str(key), value);
    }
    if let Some(buffer) = transfer {
        let list = js_sys::Array::new();
        list.push(buffer);
        let _ = window.post_message_with_transfer(&message, &origin, &list);
    } else {
        let _ = window.post_message(&message, &origin);
    }
}

fn bytes_to_buffer(bytes: &[u8]) -> js_sys::ArrayBuffer {
    let array = js_sys::Uint8Array::from(bytes);
    array.buffer().slice_with_end(
        array.byte_offset(),
        array.byte_offset() + array.byte_length(),
    )
}

fn message_kind(data: &JsValue, app: &str) -> Option<String> {
    let ty = string_field(data, "type")?;
    let rest = ty.strip_prefix(app)?.strip_prefix(':')?;
    Some(rest.to_string())
}

fn string_field(data: &JsValue, key: &str) -> Option<String> {
    js_sys::Reflect::get(data, &JsValue::from_str(key))
        .ok()?
        .as_string()
}

fn bool_field(data: &JsValue, key: &str) -> Option<bool> {
    js_sys::Reflect::get(data, &JsValue::from_str(key))
        .ok()?
        .as_bool()
}

fn bytes_field(data: &JsValue) -> Option<Vec<u8>> {
    let value = js_sys::Reflect::get(data, &JsValue::from_str("bytes")).ok()?;
    if let Ok(array) = value.clone().dyn_into::<js_sys::Uint8Array>() {
        return Some(array.to_vec());
    }
    if value.is_instance_of::<js_sys::ArrayBuffer>() {
        return Some(js_sys::Uint8Array::new(&value).to_vec());
    }
    None
}

#[component]
pub fn EditorFrame() -> impl IntoView {
    let ui = ui();
    let editor = ui.editor;
    let src = move || {
        if editor.open.get() {
            iframe_src(
                &editor.app.get(),
                &editor.author.get(),
                editor.own_author.get(),
            )
        } else {
            String::new()
        }
    };
    let pill_collapsed = RwSignal::new(crate::ui::local_flag(PILL_KEY));
    let pill_pos = RwSignal::new(saved_pill_position());
    let dragging = StoredValue::new(None::<(f64, f64, f64, f64)>);
    let on_frame_load = move |_| {
        let Some(frame) = current_frame(editor) else {
            return;
        };
        editor.frame.set_value(Some(frame.clone()));
        if is_cross_origin(&editor.app.get_untracked()) {
            return;
        }
        let Some(window) = frame.content_window() else {
            return;
        };
        crate::picker::install(&window.into(), move |id, accept, multiple| {
            ui.open_modal(Modal::Pick {
                id,
                accept,
                multiple,
            });
        });
    };
    view! {
        <Show when=move || editor.open.get()>
            <section class="editor" class:direct=move || !ui.from_drive.get()>
                <Show when=move || ui.from_drive.get()>
                    <header class="doc-bar">
                        <button type="button" class="icon-button" title=t("Back to your files") aria-label=t("Back to your files") on:click=move |_| leave_doc(None)>
                            {icon(Icon::Back)}
                        </button>
                        {move || app_icon(&editor.app.get(), "app-icon")}
                        <div class="doc-name">
                            <strong>{move || editor.name.get()}</strong>
                            <span class="muted">{move || editor.title.get()}</span>
                            {move || app_by_id(&editor.app.get()).is_some_and(|a| a.unverified).then(|| view! { <span class="badge danger">{t("Unverified")}</span> })}
                        </div>
                        <Show when=move || editor.dirty.get()><span class="dot" title=t("Unsaved changes")></span></Show>
                        <p class="status">{move || editor.status.get()}</p>
                        <div class="bar-actions">
                            <Show when=move || editor.path.get().is_some() && !editor.read_only.get()>
                                <label class="switch" title=t("Save changes to the drive a few seconds after you stop typing")>
                                    <input type="checkbox" prop:checked=move || editor.autosave_on() on:change=move |_| editor.toggle_autosave() />
                                    <span class="track"></span>
                                    {t("Autosave")}
                                </label>
                            </Show>
                            <Show when=move || editor.conflict.get().is_some()>
                                <button type="button" class="button primary" on:click=move |_| save_conflict(editor)>{t("Save a copy")}</button>
                            </Show>
                            <button type="button" class="icon-button" title=t("Close") aria-label=t("Close") on:click=move |_| leave_doc(None)>{icon(Icon::Close)}</button>
                        </div>
                    </header>
                </Show>
                <Show when=move || !ui.from_drive.get()>
                    <div class="home-pill" class:collapsed=move || pill_collapsed.get()
                        style=move || { let (x, y) = pill_pos.get(); format!("left:{x}px;top:{y}px") }>
                        <button type="button" class="grip" title=t("Drag to move") aria-label=t("Move")
                            on:pointerdown=move |ev: web_sys::PointerEvent| {
                                let (x, y) = pill_pos.get_untracked();
                                dragging.set_value(Some((ev.client_x() as f64, ev.client_y() as f64, x, y)));
                                if let Some(target) = ev.target().and_then(|t| t.dyn_into::<web_sys::Element>().ok()) {
                                    let _ = target.set_pointer_capture(ev.pointer_id());
                                }
                            }
                            on:pointermove=move |ev: web_sys::PointerEvent| {
                                if let Some((sx, sy, x, y)) = dragging.get_value() {
                                    pill_pos.set(clamp_pill(x + ev.client_x() as f64 - sx, y + ev.client_y() as f64 - sy));
                                }
                            }
                            on:pointerup=move |_| {
                                dragging.set_value(None);
                                save_pill_position(pill_pos.get_untracked());
                            }
                        >{icon(Icon::Grip)}</button>
                        <Show when=move || !pill_collapsed.get()>
                            <button type="button" class="icon-button" title=t("Your files") aria-label=t("Your files") on:click=move |_| leave_doc(None)>{icon(Icon::Home)}</button>
                            <span class="pill-name">{move || editor.name.get()}</span>
                            <Show when=move || editor.dirty.get()><span class="dot" title=t("Unsaved changes")></span></Show>
                            <Show when=move || editor.path.get().is_some() && !editor.read_only.get()>
                                <label class="switch small" title=t("Autosave")>
                                    <input type="checkbox" prop:checked=move || editor.autosave_on() on:change=move |_| editor.toggle_autosave() />
                                    <span class="track"></span>
                                </label>
                            </Show>
                        </Show>
                        <Show when=move || pill_collapsed.get()>
                            <button type="button" class="icon-button" title=t("Your files") aria-label=t("Your files") on:click=move |_| leave_doc(None)>{icon(Icon::Home)}</button>
                        </Show>
                        <button type="button" class="icon-button fold" title=move || if pill_collapsed.get() { t("Expand") } else { t("Collapse") }
                            aria-label=t("Collapse or expand")
                            on:click=move |_| {
                                let collapsed = !pill_collapsed.get_untracked();
                                pill_collapsed.set(collapsed);
                                crate::ui::set_local_flag(PILL_KEY, collapsed);
                            }>
                            <span class="chevron" class:flip=move || !pill_collapsed.get()>{icon(Icon::Chevron)}</span>
                        </button>
                    </div>
                </Show>
                <div class="doc-notes">
                    <Show when=move || !ui.from_drive.get() && !editor.status.get().is_empty()>
                        <p class="note">{move || editor.status.get()}</p>
                    </Show>
                    <Show when=move || !editor.banner.get().is_empty()>
                        <div class="note banner">
                            <span>{move || editor.banner.get()}</span>
                            <button type="button" class="icon-button" aria-label=t("Dismiss") on:click=move |_| editor.banner.set(String::new())>{icon(Icon::Close)}</button>
                        </div>
                    </Show>
                    <Show when=move || !ui.from_drive.get() && editor.conflict.get().is_some()>
                        <div class="note">
                            {t("Someone else changed this file.")}" "
                            <button type="button" class="button primary" on:click=move |_| save_conflict(editor)>{t("Save a copy")}</button>
                        </div>
                    </Show>
                </div>
                {move || {
                    let allow = iframe_allow(&editor.app.get());
                    let title = editor.title.get();
                    if is_sandboxed(&editor.app.get()) {
                        view! { <iframe src=src() sandbox="allow-scripts allow-forms allow-modals allow-downloads" on:load=on_frame_load allow=allow title=title></iframe> }.into_any()
                    } else {
                        view! { <iframe src=src() on:load=on_frame_load allow=allow title=title></iframe> }.into_any()
                    }
                }}
            </section>
        </Show>
    }
}

const PILL_KEY: &str = "artcraft-cloud:home-pill-collapsed";
const PILL_POS_KEY: &str = "artcraft-cloud:home-pill-position";

fn saved_pill_position() -> (f64, f64) {
    let stored = web_sys::window()
        .and_then(|w| w.local_storage().ok().flatten())
        .and_then(|s| s.get_item(PILL_POS_KEY).ok().flatten())
        .and_then(|v| {
            let (x, y) = v.split_once(',')?;
            Some((x.parse().ok()?, y.parse().ok()?))
        });
    // First time: bottom right, above the apps' status bars.
    let (x, y) = stored.unwrap_or_else(|| {
        let window = web_sys::window();
        let width = window
            .as_ref()
            .and_then(|w| w.inner_width().ok())
            .and_then(|v| v.as_f64())
            .unwrap_or(1200.0);
        let height = window
            .as_ref()
            .and_then(|w| w.inner_height().ok())
            .and_then(|v| v.as_f64())
            .unwrap_or(800.0);
        (width - 260.0, height - 96.0)
    });
    clamp_pill(x, y)
}

fn save_pill_position((x, y): (f64, f64)) {
    if let Some(storage) = web_sys::window().and_then(|w| w.local_storage().ok().flatten()) {
        let _ = storage.set_item(PILL_POS_KEY, &format!("{x:.0},{y:.0}"));
    }
}

fn clamp_pill(x: f64, y: f64) -> (f64, f64) {
    let window = web_sys::window();
    let width = window
        .as_ref()
        .and_then(|w| w.inner_width().ok())
        .and_then(|v| v.as_f64())
        .unwrap_or(1200.0);
    let height = window
        .as_ref()
        .and_then(|w| w.inner_height().ok())
        .and_then(|v| v.as_f64())
        .unwrap_or(800.0);
    (
        x.clamp(4.0, (width - 120.0).max(4.0)),
        y.clamp(4.0, (height - 44.0).max(4.0)),
    )
}

fn save_conflict(editor: Editor) {
    let Some(bytes) = editor.conflict.get_untracked() else {
        return;
    };
    let name = conflict_copy_name(&editor.name.get_untracked(), 1);
    let bytes = Rc::new(bytes);
    wasm_bindgen_futures::spawn_local(async move {
        if write_new(editor, &name, bytes, true).await.is_some() {
            let _ = editor.conflict.try_set(None);
        }
    });
}

pub fn confirm_save_as(editor: Editor, typed: String, folder: Option<String>) {
    let Some((fallback, bytes, rebind)) = editor.save_as.get_untracked() else {
        return;
    };
    if let Some(folder) = folder {
        let _ = editor.folder.try_set(folder);
    }
    let name = if typed.trim().is_empty() {
        fallback
    } else {
        typed.trim().to_string()
    };
    let bytes = Rc::new(bytes);
    wasm_bindgen_futures::spawn_local(async move {
        if write_new(editor, &name, bytes, rebind).await.is_some() {
            let _ = editor.save_as.try_set(None);
            ui().refresh();
        }
    });
}
