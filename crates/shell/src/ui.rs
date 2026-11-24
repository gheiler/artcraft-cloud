//! State every view shares, handed down through Leptos context.

use artcraft_cloud_types::{AppStatus, User};
use leptos::prelude::*;

use crate::editor::Editor;
use crate::route::Route;

/// What the open dialog is for. Dialogs replace every `alert`, `prompt` and `confirm`.
#[derive(Clone, Debug, PartialEq)]
pub enum Modal {
    /// One text field: a new folder in `dir`, or a new name for `path`.
    NewFolder {
        dir: String,
    },
    Rename {
        path: String,
    },
    /// Choose a folder to move or copy `path` into.
    Transfer {
        path: String,
        copy: bool,
    },
    /// Move `path` to the trash.
    Trash {
        path: String,
    },
    /// Delete one trash entry, or all of them, for good.
    Purge {
        id: Option<String>,
    },
    Share {
        path: String,
        is_dir: bool,
    },
    OpenWith {
        path: String,
        share: Option<i64>,
    },
    /// The open document has unsaved changes; going to `then` would drop them.
    Discard {
        then: Option<Route>,
    },
    /// An app asked for a file (File ▸ Open, Insert ▸ Picture, Restore backup…).
    Pick {
        id: u32,
        accept: String,
        multiple: bool,
    },
    ResetPassword {
        id: i64,
        username: String,
    },
}

#[derive(Clone, Copy)]
pub struct Ui {
    pub user: RwSignal<Option<User>>,
    pub route: RwSignal<Route>,
    pub toast: RwSignal<String>,
    pub modal: RwSignal<Option<Modal>>,
    pub apps: RwSignal<Vec<AppStatus>>,
    pub tick: RwSignal<u32>,
    /// The open document was reached from the drive (so Back returns there in history).
    pub from_drive: RwSignal<bool>,
    pub sidebar_collapsed: RwSignal<bool>,
    pub editor: Editor,
}

impl Ui {
    pub fn new() -> Self {
        Self {
            user: RwSignal::new(None),
            route: RwSignal::new(Route::Files(String::new())),
            toast: RwSignal::new(String::new()),
            modal: RwSignal::new(None),
            apps: RwSignal::new(Vec::new()),
            tick: RwSignal::new(0),
            from_drive: RwSignal::new(false),
            sidebar_collapsed: RwSignal::new(local_flag(SIDEBAR_KEY)),
            editor: Editor::new(),
        }
    }

    /// Go to `route` and add it to the browser history.
    pub fn go(&self, route: Route) {
        if self.route.get_untracked() == route {
            return;
        }
        push_url(&route.href(), false);
        let _ = self.route.try_set(route);
    }

    /// Show `route` in the address bar without a new history entry.
    pub fn replace(&self, route: Route) {
        push_url(&route.href(), true);
        let _ = self.route.try_set(route);
    }

    pub fn refresh(&self) {
        let _ = self.tick.try_update(|n| *n = n.wrapping_add(1));
    }

    pub fn say(&self, message: impl Into<String>) {
        let _ = self.toast.try_set(message.into());
    }

    pub fn open_modal(&self, modal: Modal) {
        let _ = self.modal.try_set(Some(modal));
    }

    pub fn close_modal(&self) {
        let _ = self.modal.try_set(None);
    }

    pub fn is_admin(&self) -> bool {
        self.user
            .with(|user| user.as_ref().is_some_and(|user| user.role == "admin"))
    }

    pub fn toggle_sidebar(&self) {
        let collapsed = !self.sidebar_collapsed.get_untracked();
        self.sidebar_collapsed.set(collapsed);
        set_local_flag(SIDEBAR_KEY, collapsed);
    }
}

thread_local! {
    static UI: std::cell::Cell<Option<Ui>> = const { std::cell::Cell::new(None) };
}

/// Make `ui` reachable from event listeners and async tasks outside the view tree.
pub fn set_global(ui: Ui) {
    UI.with(|slot| slot.set(Some(ui)));
}

/// The shared state. Created once by `App` before any view or listener runs.
pub fn ui() -> Ui {
    UI.with(|slot| slot.get())
        .expect("App sets the shared state first")
}

const SIDEBAR_KEY: &str = "artcraft-cloud:sidebar-collapsed";

fn push_url(href: &str, replace: bool) {
    let Some(window) = web_sys::window() else {
        return;
    };
    let Ok(history) = window.history() else {
        return;
    };
    let current = window.location().pathname().unwrap_or_default()
        + &window.location().search().unwrap_or_default();
    if current == href {
        return;
    }
    let _ = if replace {
        history.replace_state_with_url(&wasm_bindgen::JsValue::NULL, "", Some(href))
    } else {
        history.push_state_with_url(&wasm_bindgen::JsValue::NULL, "", Some(href))
    };
}

pub fn local_flag(key: &str) -> bool {
    web_sys::window()
        .and_then(|w| w.local_storage().ok().flatten())
        .and_then(|s| s.get_item(key).ok().flatten())
        .is_some_and(|v| v == "1")
}

pub fn set_local_flag(key: &str, on: bool) {
    if let Some(storage) = web_sys::window().and_then(|w| w.local_storage().ok().flatten()) {
        let _ = storage.set_item(key, if on { "1" } else { "0" });
    }
}
