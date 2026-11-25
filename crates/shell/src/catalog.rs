//! The apps this server offers, from `/api/apps` (or `/api/catalog` on a public page). The server
//! reads them from the signed app registry, so a new app shows up without a new shell.

use std::cell::Cell;

use artcraft_cloud_types::{app_for_name_in, AppStatus, CraftApp};
use leptos::prelude::*;

use crate::ui::Ui;

thread_local! {
    static APPS: Cell<&'static [CraftApp]> = const { Cell::new(&[]) };
}

/// Every app known to the page, installed or still downloading.
pub fn catalog() -> &'static [CraftApp] {
    APPS.with(Cell::get)
}

pub fn app_by_id(id: &str) -> Option<&'static CraftApp> {
    catalog().iter().find(|app| app.id == id)
}

/// The app that opens `name` by its extension.
pub fn app_for_name(name: &str) -> Option<&'static CraftApp> {
    app_for_name_in(catalog(), name)
}

/// Replace the catalog (a few kilobytes are leaked each time; it changes only after installs).
pub fn set(list: &[AppStatus]) {
    let apps: Vec<CraftApp> = list.iter().map(|app| CraftApp::leak(app)).collect();
    let apps: &'static [CraftApp] = Box::leak(apps.into_boxed_slice());
    APPS.with(|cell| cell.set(apps));
}

/// Take a fresh list from the server: the catalog first, then the signal that redraws.
pub fn adopt(ui: Ui, list: Vec<AppStatus>) {
    set(&list);
    let _ = ui.apps.try_set(list);
}
