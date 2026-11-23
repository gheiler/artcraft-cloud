//! Parent side of the `craft-web-host` contract (`?host=parent`).

use std::rc::Rc;

use artcraft_cloud_types::{app_by_id, app_for_name, conflict_copy_name, HOST_CONTRACT_VERSION};
use leptos::prelude::*;
use wasm_bindgen::{JsCast, JsValue};
use web_sys::{HtmlIFrameElement, MessageEvent};

use crate::api::{self, PutFail};

#[derive(Clone, PartialEq)]
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
    pub conflict: RwSignal<Option<Vec<u8>>>,
    pub save_as: RwSignal<Option<(String, Vec<u8>, bool)>>,
    pub frame: StoredValue<Option<HtmlIFrameElement>, LocalStorage>,
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
            conflict: RwSignal::new(None),
            save_as: RwSignal::new(None),
            frame: StoredValue::new_local(None),
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn launch(
        &self,
        app: &str,
        title: &str,
        name: String,
        path: Option<String>,
        etag: Option<String>,
        folder: String,
        kind: SaveKind,
        read_only: bool,
        bytes: Option<Vec<u8>>,
    ) {
        self.app.set(app.to_string());
        self.title.set(title.to_string());
        self.name.set(name);
        self.path.set(path);
        self.etag.set(etag);
        self.folder.set(folder);
        self.kind.set(kind);
        self.read_only.set(read_only);
        self.dirty.set(false);
        self.status.set(if bytes.is_some() { "Opening…".into() } else { "New document".into() });
        self.bytes.set(bytes);
        self.sent.set(false);
        self.conflict.set(None);
        self.save_as.set(None);
        self.open.set(true);
    }

    pub fn close(&self) {
        if self.dirty.get_untracked() {
            let discard = web_sys::window().and_then(|window| window.confirm_with_message("Discard unsaved changes?").ok()).unwrap_or(false);
            if !discard {
                return;
            }
        }
        self.dirty.set(false);
        self.open.set(false);
        self.bytes.set(None);
        self.conflict.set(None);
        self.save_as.set(None);
    }
}

pub fn install(editor: Editor) {
    let window = match web_sys::window() {
        Some(window) => window,
        None => return,
    };
    let origin = window.location().origin().unwrap_or_default();
    let on_message = wasm_bindgen::closure::Closure::wrap(Box::new(move |event: MessageEvent| {
        if event.origin() != origin {
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
            "dirty" => editor.dirty.set(bool_field(&data, "dirty").unwrap_or(false)),
            "save" => on_save(editor, &data),
            "failed" => editor.status.set(string_field(&data, "error").unwrap_or_else(|| "the app could not open the file".into())),
            "handoff" => on_handoff(editor, &data, &frame),
            _ => {}
        }
        if let Some(window) = web_sys::window() {
            let mark = if editor.dirty.get_untracked() { "● " } else { "" };
            let _ = window.document().map(|doc| doc.set_title(&format!("{mark}{} — artcraftCloud", editor.name.get_untracked())));
        }
    }) as Box<dyn FnMut(MessageEvent)>);
    let _ = window.add_event_listener_with_callback("message", on_message.as_ref().unchecked_ref());
    on_message.forget();

    let on_leave = wasm_bindgen::closure::Closure::wrap(Box::new(move |event: web_sys::BeforeUnloadEvent| {
        if editor.dirty.get_untracked() {
            event.prevent_default();
            event.set_return_value("unsaved");
        }
    }) as Box<dyn FnMut(web_sys::BeforeUnloadEvent)>);
    let _ = window.add_event_listener_with_callback("beforeunload", on_leave.as_ref().unchecked_ref());
    on_leave.forget();
}

/// The live editor iframe. `<Show>` builds a new iframe for every file opened, and the node ref
/// only reports the first one, so look it up in the document and keep the ref as a fallback.
fn current_frame(editor: Editor) -> Option<HtmlIFrameElement> {
    let live = web_sys::window()
        .and_then(|window| window.document())
        .and_then(|document| document.query_selector("section.editor iframe").ok().flatten())
        .and_then(|element| element.dyn_into::<HtmlIFrameElement>().ok());
    live.or_else(|| editor.frame.try_with_value(|frame| frame.clone()).flatten())
}

fn on_ready(editor: Editor, frame: &HtmlIFrameElement) {
    if editor.sent.get_untracked() {
        return;
    }
    post(frame, &editor.app.get_untracked(), "host", &[("version", JsValue::from_str(HOST_CONTRACT_VERSION)), ("handoff", JsValue::TRUE)], None);
    if let Some(bytes) = editor.bytes.try_update(|slot| slot.take()).flatten() {
        let name = editor.name.get_untracked();
        let buffer = bytes_to_buffer(&bytes);
        post(frame, &editor.app.get_untracked(), "open", &[("name", JsValue::from_str(&name)), ("bytes", buffer.clone().into())], Some(&buffer));
        editor.status.set(String::new());
    }
    editor.sent.set(true);
}

fn on_save(editor: Editor, data: &JsValue) {
    let name = string_field(data, "name").unwrap_or_else(|| editor.name.get_untracked());
    let save_as = bool_field(data, "saveAs").unwrap_or(false);
    let Some(bytes) = bytes_field(data) else {
        editor.status.set("The app sent a save with no bytes.".into());
        return;
    };
    if editor.read_only.get_untracked() {
        editor.status.set("View only — the file was not stored. The app has no way to hear that.".into());
        return;
    }
    let current = editor.path.get_untracked();
    let same_file = current.as_deref().is_some_and(|path| file_name(path) == name);
    if current.is_some() && !save_as && same_file {
        let bytes = Rc::new(bytes);
        wasm_bindgen_futures::spawn_local(async move {
            overwrite(editor, bytes).await;
        });
    } else if save_as || current.is_none() {
        editor.save_as.set(Some((name, bytes, true)));
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
        editor.status.set(format!("No installed app opens {name}."));
        return;
    };
    post(
        frame,
        &editor.app.get_untracked(),
        "handoff-done",
        &[("name", JsValue::from_str(&name)), ("app", JsValue::from_str(app.id))],
        None,
    );
    let bytes = Rc::new(bytes);
    let app_id = app.id.to_string();
    let title = app.title.to_string();
    let name_for_task = name.clone();
    wasm_bindgen_futures::spawn_local(async move {
        if write_new(editor, &name_for_task, Rc::clone(&bytes), false).await.is_some() {
            editor.bytes.set(Some((*bytes).clone()));
            editor.app.set(app_id);
            editor.title.set(title);
            editor.name.set(name_for_task);
            editor.sent.set(false);
            editor.status.set("Handed off.".into());
        }
    });
}

async fn overwrite(editor: Editor, bytes: Rc<Vec<u8>>) -> Option<()> {
    let path = editor.path.get_untracked()?;
    let url = content_url(editor, &relative_of(editor, &path));
    let if_match = editor.etag.get_untracked().map(|etag| format!("\"{etag}\""));
    match api::put_bytes(&url, &bytes, if_match.as_deref()).await {
        Ok(written) => {
            editor.etag.set(Some(written.etag));
            editor.dirty.set(false);
            editor.status.set("Saved.".into());
            Some(())
        }
        Err(PutFail::Conflict(etag)) => {
            if let Some(etag) = etag {
                editor.etag.set(Some(etag));
            }
            editor.conflict.set(Some((*bytes).clone()));
            editor.status.set("Someone else changed this file. Save a copy to keep your version.".into());
            None
        }
        Err(PutFail::Other(err)) => {
            editor.status.set(err);
            None
        }
    }
}

async fn write_new(editor: Editor, name: &str, bytes: Rc<Vec<u8>>, rebind: bool) -> Option<String> {
    for n in 0..20 {
        let candidate = if n == 0 { name.to_string() } else { conflict_copy_name(name, n) };
        let full = join_folder(&editor.folder.get_untracked(), &candidate);
        let url = content_url(editor, &relative_of(editor, &full));
        match api::put_bytes(&url, &bytes, None).await {
            Ok(written) => {
                editor.status.set(format!("Saved {candidate}."));
                if rebind || editor.path.get_untracked().is_none() {
                    editor.path.set(Some(full.clone()));
                    editor.name.set(candidate);
                    editor.etag.set(Some(written.etag));
                    editor.dirty.set(false);
                }
                return Some(full);
            }
            Err(PutFail::Conflict(_)) if n == 0 => continue,
            Err(PutFail::Conflict(_)) => continue,
            Err(PutFail::Other(err)) => {
                editor.status.set(err);
                return None;
            }
        }
    }
    editor.status.set("Could not find a free name.".into());
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

pub fn iframe_src(app: &str, author: &str) -> String {
    let mut url = format!("/apps/{app}/index.html?host=parent");
    if !author.trim().is_empty() && app_by_id(app).is_some_and(|item| item.author_query) {
        url.push_str("&author=");
        url.push_str(&api::enc(author.trim()));
    }
    if app_by_id(app).is_some_and(|item| item.chrome_query) {
        url.push_str("&chrome=nofiletab");
    }
    url
}

fn post(frame: &HtmlIFrameElement, app: &str, kind: &str, fields: &[(&str, JsValue)], transfer: Option<&js_sys::ArrayBuffer>) {
    let Some(window) = frame.content_window() else {
        return;
    };
    let Ok(origin) = web_sys::window().and_then(|w| w.location().origin().ok()).ok_or(()) else {
        return;
    };
    let message = js_sys::Object::new();
    let _ = js_sys::Reflect::set(&message, &JsValue::from_str("type"), &JsValue::from_str(&format!("{app}:{kind}")));
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
    array.buffer().slice_with_end(array.byte_offset(), array.byte_offset() + array.byte_length())
}

fn message_kind(data: &JsValue, app: &str) -> Option<String> {
    let ty = string_field(data, "type")?;
    let rest = ty.strip_prefix(app)?.strip_prefix(':')?;
    Some(rest.to_string())
}

fn string_field(data: &JsValue, key: &str) -> Option<String> {
    js_sys::Reflect::get(data, &JsValue::from_str(key)).ok()?.as_string()
}

fn bool_field(data: &JsValue, key: &str) -> Option<bool> {
    js_sys::Reflect::get(data, &JsValue::from_str(key)).ok()?.as_bool()
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
pub fn EditorFrame(editor: Editor, refresh: Callback<()>) -> impl IntoView {
    let iframe_ref = NodeRef::<leptos::html::Iframe>::new();
    iframe_ref.on_load(move |frame: HtmlIFrameElement| {
        editor.frame.set_value(Some(frame));
    });
    let src = move || {
        if editor.open.get() {
            iframe_src(&editor.app.get(), &editor.author.get())
        } else {
            String::new()
        }
    };
    let save_name = RwSignal::new(String::new());
    view! {
        <Show when=move || editor.open.get()>
            <section class="editor">
                <header class="editor-bar">
                    <div>
                        <strong>{move || editor.title.get()}</strong>
                        <span class="muted">" · "{move || editor.name.get()}</span>
                        <Show when=move || editor.dirty.get()><span class="dot" title="Unsaved">" ●"</span></Show>
                    </div>
                    <p class="status">{move || editor.status.get()}</p>
                    <div class="bar-actions">
                        <Show when=move || editor.conflict.get().is_some()>
                            <button type="button" on:click=move |_| save_conflict(editor)>"Save a copy"</button>
                        </Show>
                        <button type="button" on:click=move |_| editor.close()>"Close"</button>
                    </div>
                </header>
                <Show when=move || editor.save_as.get().is_some()>
                    <form class="save-as" on:submit=move |ev| {
                        ev.prevent_default();
                        confirm_save_as(editor, save_name.get_untracked(), refresh);
                    }>
                        <label>"Save into "{move || if editor.folder.get().is_empty() { "/".into() } else { editor.folder.get() }}</label>
                        <input prop:value=move || {
                            if save_name.get().is_empty() {
                                editor.save_as.with_untracked(|slot| slot.as_ref().map(|(name, _, _)| name.clone()).unwrap_or_default())
                            } else {
                                save_name.get()
                            }
                        } on:input=move |ev| save_name.set(event_target_value(&ev)) />
                        <button type="submit">"Save"</button>
                        <button type="button" on:click=move |_| editor.save_as.set(None)>"Cancel"</button>
                    </form>
                </Show>
                <iframe node_ref=iframe_ref src=src allow="fullscreen; clipboard-read; clipboard-write" title=move || editor.title.get()></iframe>
            </section>
        </Show>
    }
}

fn save_conflict(editor: Editor) {
    let Some(bytes) = editor.conflict.get_untracked() else {
        return;
    };
    let name = conflict_copy_name(&editor.name.get_untracked(), 1);
    let bytes = std::rc::Rc::new(bytes);
    wasm_bindgen_futures::spawn_local(async move {
        if write_new(editor, &name, bytes, true).await.is_some() {
            editor.conflict.set(None);
        }
    });
}

fn confirm_save_as(editor: Editor, typed: String, refresh: Callback<()>) {
    let Some((fallback, bytes, rebind)) = editor.save_as.get_untracked() else {
        return;
    };
    let name = if typed.trim().is_empty() { fallback } else { typed.trim().to_string() };
    let bytes = std::rc::Rc::new(bytes);
    wasm_bindgen_futures::spawn_local(async move {
        if write_new(editor, &name, bytes, rebind).await.is_some() {
            editor.save_as.set(None);
            refresh.run(());
        }
    });
}
