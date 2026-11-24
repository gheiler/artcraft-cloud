//! Settings: the user's theme, language, name in documents, units, animations and autosave.
//!
//! Kept per user by the server (`/api/preferences`). A change is written into every app's stored
//! options there (`artcraft_cloud_types::prefs`), so each app takes it the next time it opens;
//! the shell applies the theme at once and reloads for a new language.

use artcraft_cloud_types::catalog;
use artcraft_cloud_types::prefs::{self, Field, Preferences, LANGUAGES};
use leptos::prelude::*;

use crate::api;
use crate::i18n::{self, t, tf};
use crate::icons::app_icon;
use crate::ui::ui;

const THEME_KEY: &str = "artcraft-cloud:theme";
const LANGUAGE_KEY: &str = "artcraft-cloud:language";

fn storage() -> Option<web_sys::Storage> {
    web_sys::window()?.local_storage().ok().flatten()
}

fn cached(key: &str) -> Option<String> {
    storage()?.get_item(key).ok().flatten()
}

fn cache(key: &str, value: &str) {
    if let Some(storage) = storage() {
        let _ = storage.set_item(key, value);
    }
}

/// Before the first view: the theme and language this browser last used, so the sign-in page
/// and the first frame already look right.
// Called from `start`, which only exists in the wasm build.
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub fn boot() {
    apply_theme(&cached(THEME_KEY).unwrap_or_else(|| "system".into()));
    let preference = cached(LANGUAGE_KEY).unwrap_or_else(|| "auto".into());
    i18n::set_language(i18n::resolve(&preference, &i18n::browser_languages()));
    set_document_language();
}

fn set_document_language() {
    if let Some(root) = web_sys::window()
        .and_then(|w| w.document())
        .and_then(|d| d.document_element())
    {
        let _ = root.set_attribute("lang", i18n::language());
    }
}

/// `system`, `light` or `dark` on `<html data-theme>`; the style sheet does the rest.
pub fn apply_theme(theme: &str) {
    if let Some(root) = web_sys::window()
        .and_then(|w| w.document())
        .and_then(|d| d.document_element())
    {
        let _ = root.set_attribute("data-theme", theme);
    }
    cache(THEME_KEY, theme);
}

fn reload() {
    if let Some(window) = web_sys::window() {
        let _ = window.location().reload();
    }
}

/// The user's Settings arrived (after sign-in): use them. A language other than the one shown
/// reloads the page once, in that language.
pub fn adopt(next: Preferences) {
    let ui = ui();
    apply_theme(&next.theme);
    let name = ui
        .user
        .with_untracked(|u| u.as_ref().map(|u| u.display_name.clone()))
        .unwrap_or_default();
    ui.editor.author.set(next.author_or(&name).to_string());
    let wanted = i18n::resolve(&next.language, &i18n::browser_languages());
    cache(LANGUAGE_KEY, &next.language);
    let _ = ui.prefs.try_set(next);
    if wanted != i18n::language() {
        reload();
    }
}

/// Store `next`; the server writes the changed settings into every app's options.
pub fn save(next: Preferences) {
    let ui = ui();
    let next = next.normalized();
    apply_theme(&next.theme);
    let _ = ui.prefs.try_set(next.clone());
    wasm_bindgen_futures::spawn_local(async move {
        match api::put_preferences(&next).await {
            Ok(stored) => {
                ui.say(t("Saved. Each app uses it the next time it opens."));
                adopt(stored);
            }
            Err(err) => ui.say(tf("Could not save the settings: {error}", &[("error", &err)])),
        }
    });
}

/// The apps a setting reaches, as small icons.
fn reaches(field: Option<Field>) -> impl IntoView {
    let apps: Vec<(&'static str, &'static str)> = catalog()
        .iter()
        .filter(|app| field.is_none_or(|f| prefs::app_takes(app.id, f) || (f == Field::Author && app.author_query)))
        .map(|app| (app.id, app.title))
        .collect();
    let none = apps.is_empty();
    view! {
        <div class="reaches">
            <span class="muted">{if none { t("No app takes this from the drive yet.") } else { t("Applies to") }}</span>
            {apps.into_iter().map(|(id, title)| view! { <span class="reach" title=title>{app_icon(id, "reach-icon")}</span> }).collect_view()}
        </div>
    }
}

/// The apps that show `code`, as icons.
fn language_apps(code: String) -> impl IntoView {
    let apps: Vec<(&'static str, &'static str)> = catalog()
        .iter()
        .filter(|app| code == "auto" || (prefs::app_has_language(app.id, &code) && prefs::app_takes(app.id, Field::Language)))
        .map(|app| (app.id, app.title))
        .collect();
    view! {
        <div class="reaches">
            <span class="muted">{t("Applies to")}</span>
            {apps.into_iter().map(|(id, title)| view! { <span class="reach" title=title>{app_icon(id, "reach-icon")}</span> }).collect_view()}
        </div>
    }
}

fn choice(value: &'static str, label: &'static str, current: Signal<String>, pick: impl Fn(String) + Copy + 'static) -> impl IntoView {
    view! {
        <label class="segment" class:on=move || current.get() == value>
            <input type="radio" name="theme" value=value prop:checked=move || current.get() == value on:change=move |_| pick(value.to_string()) />
            <span>{label}</span>
        </label>
    }
}

/// An `<option>` marked selected from the signal (a `value` on the `<select>` is set before its
/// options exist).
fn option(value: &'static str, label: &'static str, current: Signal<String>) -> impl IntoView {
    view! { <option value=value prop:selected=move || current.get() == value>{label}</option> }
}

#[component]
pub fn Settings() -> impl IntoView {
    let ui = ui();
    let prefs = ui.prefs;
    let update = move |change: fn(&mut Preferences, String), value: String| {
        let mut next = prefs.get_untracked();
        change(&mut next, value);
        if next != prefs.get_untracked() {
            save(next);
        }
    };
    let theme = Signal::derive(move || prefs.with(|p| p.theme.clone()));
    let animations = Signal::derive(move || prefs.with(|p| p.animations.clone()));
    let language = Signal::derive(move || prefs.with(|p| p.language.clone()));
    let units = Signal::derive(move || prefs.with(|p| p.units.clone()));
    let account = move || ui.user.with(|u| u.as_ref().map(|u| u.display_name.clone()).unwrap_or_default());
    let shell_note = move || {
        let code = prefs.with(|p| p.language.clone());
        let shown = i18n::resolve(&code, &i18n::browser_languages());
        if code != "auto" && shown == "en" && code != "en" {
            t("The drive is not translated into this language yet and shows English; the apps that have it use it.")
        } else {
            t("The drive and the apps that have this language use it.")
        }
    };
    view! {
        <section class="drive settings-page">
            <header class="toolbar"><nav class="crumbs"><span class="crumb">{t("Settings")}</span></nav></header>
            <p class="muted intro">{t("These settings are yours on every browser. A change is written into each app's own options and takes effect the next time the app opens.")}</p>

            <div class="card settings-card">
                <h2>{t("Appearance")}</h2>
                <div class="setting">
                    <div class="setting-text"><strong>{t("Theme")}</strong><span class="muted">{t("Light, dark, or the same as your system.")}</span></div>
                    <div class="segments" role="radiogroup" aria-label=t("Theme")>
                        {choice("system", t("System"), theme, move |v| update(|p, v| p.theme = v, v))}
                        {choice("light", t("Light"), theme, move |v| update(|p, v| p.theme = v, v))}
                        {choice("dark", t("Dark"), theme, move |v| update(|p, v| p.theme = v, v))}
                    </div>
                </div>
                {reaches(Some(Field::Theme))}
                <div class="setting">
                    <div class="setting-text"><strong>{t("Animations")}</strong><span class="muted">{t("Fades and smooth scrolling in the apps.")}</span></div>
                    <select class="field narrow" aria-label=t("Animations")
                        on:change=move |ev| update(|p, v| p.animations = v, event_target_value(&ev))>
                        {option("system", t("Same as the system"), animations)}
                        {option("on", t("On"), animations)}
                        {option("off", t("Off"), animations)}
                    </select>
                </div>
                {reaches(Some(Field::Animations))}
            </div>

            <div class="card settings-card">
                <h2>{t("Language")}</h2>
                <div class="setting">
                    <div class="setting-text"><strong>{t("Interface language")}</strong><span class="muted">{shell_note}</span></div>
                    <select class="field" aria-label=t("Interface language")
                        on:change=move |ev| update(|p, v| p.language = v, event_target_value(&ev))>
                        {option("auto", t("Same as the browser"), language)}
                        {LANGUAGES.iter().map(|l| option(l.code, l.name, language)).collect_view()}
                    </select>
                </div>
                {move || language_apps(prefs.with(|p| p.language.clone()))}
            </div>

            <div class="card settings-card">
                <h2>{t("Your name in documents")}</h2>
                <div class="setting">
                    <div class="setting-text"><strong>{t("Name")}</strong><span class="muted">{t("Shown on comments, tracked changes and document properties.")}</span></div>
                    <input class="field" maxlength="80" placeholder=account prop:value=move || prefs.with(|p| p.author.clone())
                        on:change=move |ev| update(|p, v| p.author = v, event_target_value(&ev)) />
                </div>
                {reaches(Some(Field::Author))}
                <div class="setting">
                    <div class="setting-text"><strong>{t("Initials")}</strong><span class="muted">{t("Shown next to your comments.")}</span></div>
                    <input class="field narrow" maxlength="8" placeholder=move || prefs::initials_of(&prefs.with(|p| p.author_or(&account()).to_string()))
                        prop:value=move || prefs.with(|p| p.initials.clone())
                        on:change=move |ev| update(|p, v| p.initials = v, event_target_value(&ev)) />
                </div>
                {reaches(Some(Field::Initials))}
            </div>

            <div class="card settings-card">
                <h2>{t("Documents")}</h2>
                <div class="setting">
                    <div class="setting-text"><strong>{t("Measurement units")}</strong><span class="muted">{t("For rulers, margins and sizes.")}</span></div>
                    <select class="field narrow" aria-label=t("Measurement units")
                        on:change=move |ev| update(|p, v| p.units = v, event_target_value(&ev))>
                        {option("auto", t("Each app's default"), units)}
                        {option("inches", t("Inches"), units)}
                        {option("centimeters", t("Centimeters"), units)}
                        {option("millimeters", t("Millimeters"), units)}
                        {option("points", t("Points"), units)}
                    </select>
                </div>
                {reaches(Some(Field::Units))}
                <div class="setting">
                    <div class="setting-text"><strong>{t("Autosave")}</strong><span class="muted">{t("Save changes to the drive a few seconds after you stop typing")}</span></div>
                    <label class="switch">
                        <input type="checkbox" prop:checked=move || prefs.with(|p| p.autosave)
                            on:change=move |ev| {
                                let on = event_target_checked(&ev);
                                let mut next = prefs.get_untracked();
                                next.autosave = on;
                                ui.editor.set_autosave_off(Vec::new());
                                save(next);
                            } />
                        <span class="track"></span>
                    </label>
                </div>
                {reaches(None)}
            </div>
        </section>
    }
}
