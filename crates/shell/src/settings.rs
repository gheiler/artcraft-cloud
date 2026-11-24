//! Per-user app options, kept by the server.
//!
//! The apps keep their options (WordCraft's author and initials, GridCraft's AutoSave and view
//! preferences, CADCraft's profile…) in this origin's `localStorage`, under keys that start with the
//! app id (`wordcraft/…`, `gridcraft.prefs`, `pdfcraft`, `cadcraft.profile`). That storage belongs
//! to the browser, not to the person, so the shell:
//!
//! 1. before an app starts, replaces the app's keys with the copy saved for the signed-in user
//!    (`GET /api/settings/<app>`), so the app boots with their options on any browser;
//! 2. listens for `storage` events (fired in this window when the same-origin app frame writes
//!    `localStorage`) and saves the app's keys back a moment later (`PUT /api/settings/<app>`),
//!    and once more when the document closes.
//!
//! The apps have no hook for this (see docs/upstream-gaps.md); options an app keeps elsewhere
//! (IndexedDB, OPFS) are not carried.

use std::cell::{Cell, RefCell};

use serde_json::{Map, Value};
use wasm_bindgen::{closure::Closure, JsCast};
use leptos::prelude::*;

use artcraft_cloud_types::prefs::{self, Field, Mode};

use crate::api;

/// Shared egui state that no single app owns.
const SHARED_KEYS: &[&str] = &["egui_memory_ron"];
/// Wait this long after the last write before saving.
const DEBOUNCE_MS: i32 = 1500;
/// Largest settings object the server takes.
const LIMIT: usize = 1024 * 1024;

thread_local! {
    static CURRENT: RefCell<String> = const { RefCell::new(String::new()) };
    static GENERATION: Cell<u32> = const { Cell::new(0) };
    static LAST_SAVED: RefCell<String> = const { RefCell::new(String::new()) };
}

/// Whether `key` in `localStorage` belongs to `app`.
pub fn owned_by(app: &str, key: &str) -> bool {
    if app.is_empty() || SHARED_KEYS.contains(&key) {
        return false;
    }
    match key.strip_prefix(app) {
        Some("") => true,
        Some(rest) => rest.starts_with(['.', '/', ':', '-', '_']),
        None => false,
    }
}

fn storage() -> Option<web_sys::Storage> {
    web_sys::window()?.local_storage().ok().flatten()
}

/// The app's keys as they are now.
pub fn snapshot(app: &str) -> Map<String, Value> {
    let mut out = Map::new();
    let Some(storage) = storage() else { return out };
    let len = storage.length().unwrap_or(0);
    for index in 0..len {
        let Some(key) = storage.key(index).ok().flatten() else {
            continue;
        };
        if owned_by(app, &key) {
            if let Some(value) = storage.get_item(&key).ok().flatten() {
                out.insert(key, Value::String(value));
            }
        }
    }
    out
}

/// Put the user's saved options for `app` in place before the app starts. Returns whether the
/// saved options name an author (then the shell does not pass `?author=` over it).
pub async fn restore(app: &str, user: i64) -> bool {
    CURRENT.with(|current| *current.borrow_mut() = app.to_string());
    let saved = match api::settings(app).await {
        Ok(saved) => saved,
        Err(_) => return false,
    };
    let Some(storage) = storage() else {
        return false;
    };
    // Options left in this browser by someone else are not theirs to start with. The signed-in
    // user's own local options (or unmarked ones from before) are kept once, when the server has none
    // yet, and uploaded.
    let owner_key = format!("artcraft-cloud:options-owner:{app}");
    let local_owner = storage.get_item(&owner_key).ok().flatten();
    // A browser used before this feature has no owner mark: its options go to whoever opens the app first.
    let keep_local = saved.is_empty()
        && local_owner
            .as_deref()
            .is_none_or(|owner| owner == user.to_string());
    if !keep_local {
        let stale: Vec<String> = snapshot(app).keys().cloned().collect();
        for key in stale {
            let _ = storage.remove_item(&key);
        }
    }
    let _ = storage.set_item(&owner_key, &user.to_string());
    for (key, value) in &saved {
        if !owned_by(app, key) {
            continue;
        }
        let Some(text) = value.as_str() else { continue };
        let _ = storage.set_item(key, &forget_recent(text));
    }
    // What the server has (or, the first time, nothing): a save after this uploads whatever
    // the app or the settings below change.
    LAST_SAVED.with(|last| {
        *last.borrow_mut() = if keep_local {
            String::new()
        } else {
            Value::Object(snapshot(app)).to_string()
        }
    });
    fill_from_settings(app, &storage);
    flush();
    snapshot(app)
        .values()
        .filter_map(Value::as_str)
        .any(names_author)
}

/// Options the app has not chosen yet (no language, its placeholder name…) start from the user's
/// Settings, so a first start is already in their language and under their name.
fn fill_from_settings(app: &str, storage: &web_sys::Storage) {
    let ui = crate::ui::ui();
    let prefs = ui.prefs.get_untracked();
    let name = ui
        .user
        .with_untracked(|user| user.as_ref().map(|u| u.display_name.clone()))
        .unwrap_or_default();
    let mut now = snapshot(app);
    let before = now.clone();
    if prefs::apply(app, &prefs, &Field::ALL, &name, Mode::FillMissing, &mut now) {
        for (key, value) in &now {
            if before.get(key) != Some(value) {
                if let Some(text) = value.as_str() {
                    let _ = storage.set_item(key, text);
                }
            }
        }
    }
}

/// Save the open app's keys now, when they changed since the last save.
pub fn flush() {
    let app = CURRENT.with(|current| current.borrow().clone());
    if app.is_empty() {
        return;
    }
    let now = snapshot(&app);
    let text = Value::Object(now.clone()).to_string();
    if text.len() > LIMIT || LAST_SAVED.with(|last| *last.borrow() == text) {
        return;
    }
    LAST_SAVED.with(|last| *last.borrow_mut() = text);
    wasm_bindgen_futures::spawn_local(async move {
        if let Err(err) = api::put_settings(&app, &now).await {
            web_sys::console::warn_1(
                &format!("artcraftCloud: could not save {app} options: {err}").into(),
            );
        }
    });
}

/// Stop following an app (its document closed), after one last save.
pub fn leave() {
    flush();
    CURRENT.with(|current| current.borrow_mut().clear());
}

/// Listen for the app frame's `localStorage` writes and save them shortly after.
pub fn install() {
    let Some(window) = web_sys::window() else {
        return;
    };
    let on_storage =
        Closure::<dyn FnMut(web_sys::StorageEvent)>::new(move |event: web_sys::StorageEvent| {
            let app = CURRENT.with(|current| current.borrow().clone());
            let Some(key) = event.key() else { return };
            if !owned_by(&app, &key) {
                return;
            }
            let generation = GENERATION.with(|g| {
                g.set(g.get().wrapping_add(1));
                g.get()
            });
            let later = Closure::once_into_js(move || {
                if GENERATION.with(|g| g.get()) == generation {
                    flush();
                }
            });
            if let Some(window) = web_sys::window() {
                let _ = window.set_timeout_with_callback_and_timeout_and_arguments_0(
                    later.unchecked_ref(),
                    DEBOUNCE_MS,
                );
            }
        });
    let _ = window.add_event_listener_with_callback("storage", on_storage.as_ref().unchecked_ref());
    on_storage.forget();
    let on_hide = Closure::<dyn FnMut()>::new(flush);
    let _ = window.add_event_listener_with_callback("pagehide", on_hide.as_ref().unchecked_ref());
    on_hide.forget();
}

/// Recent-file lists hold paths the browser build cannot open ("files are opened through the
/// browser on the web"), so a restored list starts empty. Files open from the drive instead.
pub fn forget_recent(text: &str) -> String {
    let Ok(mut value) = serde_json::from_str::<Value>(text) else {
        return text.to_string();
    };
    if !clear_recent(&mut value) {
        return text.to_string();
    }
    value.to_string()
}

fn clear_recent(value: &mut Value) -> bool {
    let mut changed = false;
    match value {
        Value::Object(map) => {
            for (key, inner) in map.iter_mut() {
                if (key == "recent" || key == "recent_files" || key == "recentFiles")
                    && inner.as_array().is_some_and(|list| !list.is_empty())
                {
                    *inner = Value::Array(Vec::new());
                    changed = true;
                } else {
                    changed |= clear_recent(inner);
                }
            }
        }
        Value::String(inner) if inner.starts_with('{') => {
            // Some stores keep JSON inside a JSON string.
            let cleaned = forget_recent(inner);
            if cleaned != *inner {
                *inner = cleaned;
                changed = true;
            }
        }
        _ => {}
    }
    changed
}

fn names_author(text: &str) -> bool {
    fn walk(value: &Value) -> bool {
        match value {
            Value::Object(map) => map.iter().any(|(key, inner)| {
                (key == "author" && inner.as_str().is_some_and(|s| !s.trim().is_empty()))
                    || walk(inner)
            }),
            Value::Array(list) => list.iter().any(walk),
            Value::String(inner) if inner.starts_with('{') => serde_json::from_str::<Value>(inner)
                .map(|v| walk(&v))
                .unwrap_or(false),
            _ => false,
        }
    }
    serde_json::from_str::<Value>(text)
        .map(|v| walk(&v))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_are_matched_to_their_app() {
        assert!(owned_by("pdfcraft", "pdfcraft"));
        assert!(owned_by("wordcraft", "wordcraft/settings.json"));
        assert!(owned_by("gridcraft", "gridcraft.prefs"));
        assert!(owned_by("cadcraft", "cadcraft.panel-layout"));
        assert!(!owned_by("cadcraft", "cadcraftx"));
        assert!(!owned_by("wordcraft", "gridcraft.ui"));
        assert!(!owned_by("pdfcraft", "egui_memory_ron"));
        assert!(!owned_by("", "anything"));
    }

    #[test]
    fn recent_lists_are_emptied_and_authors_found() {
        let text =
            r#"{"author":"Gabriel","initials":"GH","recent":["/Hola.docx"],"theme":"light"}"#;
        let cleaned: Value = serde_json::from_str(&forget_recent(text)).unwrap();
        assert_eq!(cleaned["recent"], serde_json::json!([]));
        assert_eq!(cleaned["initials"], "GH");
        assert!(names_author(text));
        assert!(!names_author(r#"{"author":"  "}"#));
        assert!(!names_author("not json"));
        assert_eq!(forget_recent("plain"), "plain");
        let nested = r#"{"data":"{\"recent\":[\"a\"]}"}"#;
        assert!(forget_recent(nested).contains(r#"\"recent\":[]"#));
    }
}
