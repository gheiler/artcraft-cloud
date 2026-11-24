//! The user's common settings (Settings in the shell) and how each one is written into the
//! options every Crafting App keeps for itself.
//!
//! The apps have no settings message in the host contract. Each keeps its options in the page's
//! `localStorage` (see docs/upstream-gaps.md), and the shell stores a copy of those keys per user
//! (`/api/settings/<app>`, a JSON object of key → stored text). This module edits that copy:
//!
//! - [`Mode::Override`] when a setting changes in Settings: the new value replaces the app's own,
//!   so it takes effect the next time the app opens;
//! - [`Mode::FillMissing`] when an app starts: values the app has not stored yet (no language,
//!   the placeholder author name…) are filled in from Settings.
//!
//! [`targets`] is the mapping, checked against the released web builds in the catalog. A value an
//! app cannot show (a language it has no catalog for) is never written: the app keeps its own.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// The common settings. Every field is a plain string so stored values from a newer or older
/// shell never fail to load; [`Preferences::normalized`] maps unknown values to the defaults.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Preferences {
    /// `system`, `light` or `dark`.
    pub theme: String,
    /// `auto` (follow the browser) or a code from [`LANGUAGES`].
    pub language: String,
    /// Name for comments, tracked changes and document properties; empty: the account's name.
    pub author: String,
    /// Initials for comments; empty: taken from the name.
    pub initials: String,
    /// `auto` (each app's default for the region), `inches`, `centimeters`, `millimeters`, `points`.
    pub units: String,
    /// `system`, `on` or `off`.
    pub animations: String,
    /// Save open documents automatically (the shell's own autosave, every app).
    pub autosave: bool,
}

impl Default for Preferences {
    fn default() -> Self {
        Preferences {
            theme: "system".into(),
            language: "auto".into(),
            author: String::new(),
            initials: String::new(),
            units: "auto".into(),
            animations: "system".into(),
            autosave: true,
        }
    }
}

pub const THEMES: &[&str] = &["system", "light", "dark"];
pub const UNITS: &[&str] = &["auto", "inches", "centimeters", "millimeters", "points"];
pub const ANIMATIONS: &[&str] = &["system", "on", "off"];
/// Longest name or initials kept (the apps cap their own fields similarly).
pub const MAX_NAME: usize = 80;
pub const MAX_INITIALS: usize = 8;

impl Preferences {
    /// The same settings with unknown values replaced by defaults and names trimmed.
    pub fn normalized(&self) -> Preferences {
        let pick = |value: &str, allowed: &[&str]| {
            let value = value.trim().to_ascii_lowercase();
            if allowed.contains(&value.as_str()) { value } else { allowed[0].to_string() }
        };
        let language = self.language.trim().to_ascii_lowercase();
        Preferences {
            theme: pick(&self.theme, THEMES),
            language: if language_by_code(&language).is_some() { language } else { "auto".into() },
            author: self.author.trim().chars().take(MAX_NAME).collect(),
            initials: self.initials.trim().chars().take(MAX_INITIALS).collect(),
            units: pick(&self.units, UNITS),
            animations: pick(&self.animations, ANIMATIONS),
            autosave: self.autosave,
        }
    }

    /// The name written into the apps: the chosen one, else the account's.
    pub fn author_or<'a>(&'a self, account_name: &'a str) -> &'a str {
        if self.author.is_empty() { account_name } else { &self.author }
    }

    /// The initials written into the apps: the chosen ones, else from the name.
    pub fn initials_or(&self, account_name: &str) -> String {
        if self.initials.is_empty() { initials_of(self.author_or(account_name)) } else { self.initials.clone() }
    }

    /// Which settings differ between `self` and `next`.
    pub fn changed(&self, next: &Preferences) -> Vec<Field> {
        let mut out = Vec::new();
        if self.theme != next.theme {
            out.push(Field::Theme);
        }
        if self.language != next.language {
            out.push(Field::Language);
        }
        if self.author != next.author {
            out.push(Field::Author);
        }
        if self.initials != next.initials || (self.author != next.author && next.initials.is_empty()) {
            out.push(Field::Initials);
        }
        if self.units != next.units {
            out.push(Field::Units);
        }
        if self.animations != next.animations {
            out.push(Field::Animations);
        }
        out
    }
}

/// `Gabriel Heiler` → `GH`; one word gives its first letter.
pub fn initials_of(name: &str) -> String {
    name.split_whitespace().filter_map(|w| w.chars().next()).take(2).flat_map(char::to_uppercase).collect()
}

/// A common setting the apps take.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field {
    Theme,
    Language,
    Author,
    Initials,
    Units,
    Animations,
}

impl Field {
    pub const ALL: [Field; 6] = [Field::Theme, Field::Language, Field::Author, Field::Initials, Field::Units, Field::Animations];
}

/// An interface language some Crafting App ships, with its name in that language.
#[derive(Debug, Clone, Copy)]
pub struct Language {
    /// BCP 47, lowercase, as craft-i18n names its locales (`pt-br`, `zh-hans`).
    pub code: &'static str,
    pub name: &'static str,
}

/// Every interface language of the apps in the catalog (the union), by code. Each app's own
/// list is in [`app_language`].
pub static LANGUAGES: &[Language] = &[
    Language { code: "en", name: "English" },
    Language { code: "ar", name: "العربية" },
    Language { code: "bg", name: "Български" },
    Language { code: "bs", name: "Bosanski" },
    Language { code: "ca", name: "Català" },
    Language { code: "ckb", name: "کوردی" },
    Language { code: "cs", name: "Čeština" },
    Language { code: "da", name: "Dansk" },
    Language { code: "de", name: "Deutsch" },
    Language { code: "el", name: "Ελληνικά" },
    Language { code: "es", name: "Español" },
    Language { code: "et", name: "Eesti" },
    Language { code: "fa", name: "فارسی" },
    Language { code: "fi", name: "Suomi" },
    Language { code: "fr", name: "Français" },
    Language { code: "hi", name: "हिन्दी" },
    Language { code: "hr", name: "Hrvatski" },
    Language { code: "hu", name: "Magyar" },
    Language { code: "id", name: "Bahasa Indonesia" },
    Language { code: "it", name: "Italiano" },
    Language { code: "ja", name: "日本語" },
    Language { code: "ka", name: "ქართული" },
    Language { code: "kk", name: "Қазақ тілі" },
    Language { code: "ko", name: "한국어" },
    Language { code: "lo", name: "ລາວ" },
    Language { code: "lt", name: "Lietuvių" },
    Language { code: "nb", name: "Norsk bokmål" },
    Language { code: "nl", name: "Nederlands" },
    Language { code: "pl", name: "Polski" },
    Language { code: "pt-br", name: "Português (Brasil)" },
    Language { code: "ro", name: "Română" },
    Language { code: "ru", name: "Русский" },
    Language { code: "sl", name: "Slovenščina" },
    Language { code: "sq", name: "Shqip" },
    Language { code: "sr", name: "Српски" },
    Language { code: "sr-latn", name: "Srpski (latinica)" },
    Language { code: "sv", name: "Svenska" },
    Language { code: "te", name: "తెలుగు" },
    Language { code: "th", name: "ไทย" },
    Language { code: "tr", name: "Türkçe" },
    Language { code: "uk", name: "Українська" },
    Language { code: "vi", name: "Tiếng Việt" },
    Language { code: "zh-hans", name: "简体中文" },
    Language { code: "zh-hant", name: "繁體中文" },
];

pub fn language_by_code(code: &str) -> Option<&'static Language> {
    LANGUAGES.iter().find(|l| l.code == code)
}

// Interface languages per app, from each app's source (November 2026): WordCraft and PdfCraft
// `ui-egui/src/i18n.rs` `LANGUAGES`, GridCraft `i18n::Language`, CADCraft docs/localization.md,
// LightCraft `i18n.rs`. DeckCraft, FigCraft and FilmCraft are English only.
const WORDCRAFT: &[&str] = &[
    "en", "zh-hans", "zh-hant", "ja", "ko", "uk", "es", "pt-br", "sr", "sr-latn", "pl", "cs", "et", "de", "ru", "tr", "it", "hu",
    "fr", "bg", "vi", "sv", "hr", "nl", "da", "ro", "ar", "sl", "bs",
];
const PDFCRAFT: &[&str] = &[
    "en", "ja", "zh-hans", "zh-hant", "cs", "pt-br", "de", "es", "fr", "ru", "kk", "bg", "te", "hu", "uk", "it", "ar", "ckb", "fa",
    "pl", "sl", "bs", "el", "tr", "lt", "id", "vi", "fi", "sq", "th", "ca", "ro", "lo", "hi", "ko", "nb",
];
const GRIDCRAFT: &[(&str, &str)] = &[
    ("en", "en"), ("es", "es"), ("el", "el"), ("zh-hans", "zh"), ("ja", "ja"), ("ko", "ko"), ("ru", "ru"), ("pt-br", "pt"),
    ("de", "de"), ("cs", "cs"), ("fr", "fr"), ("sr", "sr"), ("tr", "tr"), ("pl", "pl"), ("it", "it"), ("uk", "uk"),
    ("vi", "vi"), ("nl", "nl"), ("ro", "ro"),
];
const CADCRAFT: &[(&str, &str)] = &[
    ("en", "en"), ("uk", "uk"), ("pt-br", "pt-BR"), ("de", "de"), ("es", "es"), ("cs", "cs"), ("ru", "ru"), ("fr", "fr"),
    ("zh-hans", "zh"), ("it", "it"), ("ko", "ko"), ("sv", "sv"), ("ar", "ar"), ("hu", "hu"), ("tr", "tr"), ("bs", "bs"),
    ("el", "el"), ("ka", "ka"), ("vi", "vi"),
];
// CalendarCraft `src/i18n.rs` `LANGUAGES`.
const CALENDARCRAFT: &[&str] = &["en", "de", "es", "fr", "it", "pt-br"];
const LIGHTCRAFT: &[&str] =
    &["en", "zh-hans", "zh-hant", "ja", "pt-br", "es", "de", "ru", "fr", "uk", "pl", "cs", "it", "ka", "sv", "fi", "id", "vi"];

/// The code `app` stores for interface language `code`, when the app has that language.
pub fn app_language(app: &str, code: &str) -> Option<&'static str> {
    let same = |list: &'static [&'static str]| list.iter().copied().find(|c| *c == code);
    let mapped = |list: &'static [(&'static str, &'static str)]| list.iter().find(|(c, _)| *c == code).map(|(_, v)| *v);
    match app {
        "wordcraft" => same(WORDCRAFT),
        "pdfcraft" => same(PDFCRAFT),
        "gridcraft" => mapped(GRIDCRAFT),
        "cadcraft" => mapped(CADCRAFT),
        "lightcraft" => same(LIGHTCRAFT),
        "calendarcraft" => same(CALENDARCRAFT),
        _ => (code == "en").then_some("en"),
    }
}

/// Whether `app` shows interface language `code` (it may still not take it from the host).
pub fn app_has_language(app: &str, code: &str) -> bool {
    app_language(app, code).is_some()
}

/// How a setting is applied.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Settings changed: replace what the app stored.
    Override,
    /// The app is starting: fill in only what it has not stored.
    FillMissing,
}

/// One value written into an app's stored options.
#[derive(Debug, Clone, PartialEq)]
pub struct Write {
    /// The `localStorage` key.
    pub key: &'static str,
    /// Where inside the key's JSON; empty for a key that holds plain text.
    pub path: &'static [&'static str],
    pub value: Value,
    /// Create the key when the app has not stored it yet (only for stores that read any subset).
    pub create: bool,
    /// Stored values that mean "not chosen" (the apps' placeholder names, `auto`).
    pub unset: &'static [&'static str],
}

fn w(key: &'static str, path: &'static [&'static str], value: Value) -> Write {
    Write { key, path, value, create: true, unset: &[] }
}

/// What `field` writes into `app`'s options, given the settings. Empty when the app has no such
/// option, cannot take it from storage, or has no equivalent of the value (see the README table).
pub fn targets(app: &str, field: Field, prefs: &Preferences, account_name: &str) -> Vec<Write> {
    let p = prefs.normalized();
    let theme = p.theme.as_str();
    let author = p.author_or(account_name).to_string();
    let initials = p.initials_or(account_name);
    let lang = || (p.language != "auto").then(|| app_language(app, &p.language)).flatten();
    match (app, field) {
        ("wordcraft", Field::Theme) => vec![w("wordcraft.ui", &["theme"], theme.into())],
        ("wordcraft", Field::Language) => {
            lang().map(|l| Write { unset: &["auto"], ..w("wordcraft.ui", &["language"], l.into()) }).into_iter().collect()
        }
        ("wordcraft", Field::Author) if !author.is_empty() => {
            vec![Write { unset: &["WordCraft User"], ..w("wordcraft.ui", &["author"], author.into()) }]
        }
        ("wordcraft", Field::Initials) if !initials.is_empty() => vec![w("wordcraft.ui", &["initials"], initials.into())],
        ("wordcraft", Field::Units) if p.units != "auto" => {
            vec![w("wordcraft.ui", &["editing", "measurementUnits"], p.units.clone().into())]
        }
        ("wordcraft", Field::Animations) => vec![w("wordcraft.ui", &["animations"], p.animations.clone().into())],

        ("gridcraft", Field::Theme) => vec![
            w("gridcraft.ui", &["systemTheme"], (theme == "system").into()),
            w("gridcraft.ui", &["dark"], (theme == "dark").into()),
        ],
        ("gridcraft", Field::Language) => lang().map(|l| w("gridcraft.ui", &["language"], l.into())).into_iter().collect(),
        ("gridcraft", Field::Author) if !author.is_empty() => {
            vec![Write { unset: &["GridCraft User"], ..w("gridcraft.prefs", &["userName"], author.into()) }]
        }
        ("gridcraft", Field::Animations) if p.animations != "system" => {
            vec![w("gridcraft.ui", &["animations"], (p.animations == "on").into())]
        }

        ("pdfcraft", Field::Theme) => {
            let value = match theme {
                "light" => "Light",
                "dark" => "Dark",
                _ => "System",
            };
            vec![w("pdfcraft", &["theme"], value.into())]
        }
        ("pdfcraft", Field::Language) => {
            lang().map(|l| Write { unset: &["auto"], ..w("pdfcraft", &["language"], l.into()) }).into_iter().collect()
        }
        ("pdfcraft", Field::Author) if !author.is_empty() => {
            vec![Write { unset: &["Guest"], ..w("pdfcraft", &["author"], author.into()) }]
        }
        ("pdfcraft", Field::Units) if p.units != "auto" => {
            let value = match p.units.as_str() {
                "centimeters" => "centimetres",
                "millimeters" => "millimetres",
                other => other,
            };
            vec![w("pdfcraft", &["units"], value.to_string().into())]
        }
        ("pdfcraft", Field::Animations) => vec![w("pdfcraft", &["animation_effects"], p.animations.clone().into())],

        ("cadcraft", Field::Theme) => vec![w("cadcraft.prefs", &["theme"], theme.into())],
        ("cadcraft", Field::Language) => lang()
            .map(|l| Write { unset: &["auto"], ..w("cadcraft.prefs", &["interfaceLanguage"], l.into()) })
            .into_iter()
            .collect(),
        ("cadcraft", Field::Animations) => vec![w("cadcraft.prefs", &["animations"], p.animations.clone().into())],
        ("cadcraft", Field::Units) if p.units != "auto" => {
            // MEASUREINIT: units for new drawings, 0 imperial, 1 metric. The profile is a full
            // document with a version, so it is only edited once CADCraft has written it.
            let metric = matches!(p.units.as_str(), "centimeters" | "millimeters");
            vec![Write { create: false, ..w("cadcraft.profile", &["sysvars", "MEASUREINIT"], i64::from(metric).into()) }]
        }

        ("figcraft", Field::Author) if !author.is_empty() => vec![w("figcraft.userName", &[], author.into())],

        ("calendarcraft", Field::Theme) => vec![w("calendarcraft.ui", &["theme"], theme.into())],
        ("calendarcraft", Field::Language) => {
            lang().map(|l| Write { unset: &["auto"], ..w("calendarcraft.ui", &["language"], l.into()) }).into_iter().collect()
        }
        ("calendarcraft", Field::Animations) => vec![w("calendarcraft.ui", &["animations"], p.animations.clone().into())],
        _ => Vec::new(),
    }
}

/// Whether `field` reaches `app` at all (for the Settings page).
pub fn app_takes(app: &str, field: Field) -> bool {
    let sample = Preferences {
        language: "de".into(),
        author: "A B".into(),
        initials: "AB".into(),
        units: "centimeters".into(),
        animations: "off".into(),
        ..Preferences::default()
    };
    !targets(app, field, &sample, "").is_empty()
}

/// Apply `fields` of `prefs` to `stored`, one app's options as `/api/settings/<app>` keeps them
/// (key → stored text). Returns whether anything changed.
pub fn apply(app: &str, prefs: &Preferences, fields: &[Field], account_name: &str, mode: Mode, stored: &mut Map<String, Value>) -> bool {
    let mut changed = false;
    for field in fields {
        for write in targets(app, *field, prefs, account_name) {
            changed |= apply_one(&write, mode, stored);
        }
    }
    changed
}

fn is_unset(value: Option<&Value>, unset: &[&str]) -> bool {
    match value {
        None | Some(Value::Null) => true,
        Some(Value::String(s)) => s.trim().is_empty() || unset.contains(&s.as_str()),
        _ => false,
    }
}

fn apply_one(write: &Write, mode: Mode, stored: &mut Map<String, Value>) -> bool {
    if write.path.is_empty() {
        let current = stored.get(write.key);
        if mode == Mode::FillMissing && !is_unset(current, write.unset) {
            return false;
        }
        if current == Some(&write.value) {
            return false;
        }
        stored.insert(write.key.to_string(), write.value.clone());
        return true;
    }
    let mut doc = match stored.get(write.key) {
        Some(Value::String(text)) => match serde_json::from_str::<Value>(text) {
            Ok(Value::Object(map)) => map,
            // Not ours to rewrite: an app format we do not know.
            _ => return false,
        },
        Some(_) => return false,
        None if write.create => Map::new(),
        None => return false,
    };
    let (last, parents) = match write.path.split_last() {
        Some(split) => split,
        None => return false,
    };
    let mut node = &mut doc;
    for part in parents {
        let entry = node.entry(part.to_string()).or_insert_with(|| Value::Object(Map::new()));
        if entry.is_null() {
            *entry = Value::Object(Map::new());
        }
        match entry {
            Value::Object(inner) => node = inner,
            _ => return false,
        }
    }
    let current = node.get(*last);
    if mode == Mode::FillMissing && !is_unset(current, write.unset) {
        return false;
    }
    if current == Some(&write.value) {
        return false;
    }
    node.insert(last.to_string(), write.value.clone());
    stored.insert(write.key.to_string(), Value::String(Value::Object(doc).to_string()));
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn inner(stored: &Map<String, Value>, key: &str) -> Value {
        serde_json::from_str(stored[key].as_str().unwrap()).unwrap()
    }

    fn prefs() -> Preferences {
        Preferences {
            theme: "dark".into(),
            language: "de".into(),
            author: "Gabriel Heiler".into(),
            units: "centimeters".into(),
            ..Preferences::default()
        }
    }

    #[test]
    fn unknown_values_read_as_defaults() {
        let p = Preferences { theme: "Neon".into(), language: "xx".into(), units: "furlongs".into(), ..prefs() }.normalized();
        assert_eq!((p.theme.as_str(), p.language.as_str(), p.units.as_str()), ("system", "auto", "auto"));
        let p: Preferences = serde_json::from_str(r#"{"theme":"DARK","future":1}"#).unwrap();
        assert_eq!(p.normalized().theme, "dark");
        assert_eq!(p.language, "auto", "missing fields take defaults");
    }

    #[test]
    fn initials_come_from_the_name() {
        assert_eq!(initials_of("Gabriel Heiler"), "GH");
        assert_eq!(initials_of("ana"), "A");
        assert_eq!(prefs().initials_or("x"), "GH");
        assert_eq!(Preferences::default().initials_or("María López"), "ML");
    }

    #[test]
    fn override_writes_every_app_that_takes_the_setting() {
        let mut word = Map::new();
        word.insert("wordcraft.ui".into(), json!(r#"{"theme":"system","language":"auto","author":"WordCraft User","editing":{"measurementUnits":null,"x":1},"tab":"Home"}"#));
        assert!(apply("wordcraft", &prefs(), &Field::ALL, "", Mode::Override, &mut word));
        let ui = inner(&word, "wordcraft.ui");
        assert_eq!(ui["theme"], "dark");
        assert_eq!(ui["language"], "de");
        assert_eq!(ui["author"], "Gabriel Heiler");
        assert_eq!(ui["initials"], "GH");
        assert_eq!(ui["editing"]["measurementUnits"], "centimeters");
        assert_eq!(ui["editing"]["x"], 1, "the app's other options stay");
        assert_eq!(ui["tab"], "Home");

        let mut grid = Map::new();
        assert!(apply("gridcraft", &prefs(), &Field::ALL, "", Mode::Override, &mut grid));
        assert_eq!(inner(&grid, "gridcraft.ui"), json!({"systemTheme": false, "dark": true, "language": "de"}));
        assert_eq!(inner(&grid, "gridcraft.prefs"), json!({"userName": "Gabriel Heiler"}));

        let mut pdf = Map::new();
        apply("pdfcraft", &prefs(), &Field::ALL, "", Mode::Override, &mut pdf);
        let p = inner(&pdf, "pdfcraft");
        assert_eq!((p["theme"].as_str(), p["units"].as_str(), p["language"].as_str()), (Some("Dark"), Some("centimetres"), Some("de")));

        let mut fig = Map::new();
        apply("figcraft", &prefs(), &Field::ALL, "", Mode::Override, &mut fig);
        assert_eq!(fig["figcraft.userName"], "Gabriel Heiler", "plain text, not JSON");

        let mut cal = Map::new();
        assert!(apply("calendarcraft", &prefs(), &Field::ALL, "", Mode::Override, &mut cal));
        let c = inner(&cal, "calendarcraft.ui");
        assert_eq!((c["theme"].as_str(), c["language"].as_str()), (Some("dark"), Some("de")));
        assert!(c.get("author").is_none(), "CalendarCraft has no author option");

        let mut deck = Map::new();
        assert!(!apply("deckcraft", &prefs(), &Field::ALL, "", Mode::Override, &mut deck), "DeckCraft keeps nothing in storage");
    }

    #[test]
    fn languages_an_app_lacks_are_not_written() {
        let p = Preferences { language: "zh-hant".into(), ..Preferences::default() };
        let mut grid = Map::new();
        apply("gridcraft", &p, &[Field::Language], "", Mode::Override, &mut grid);
        assert!(grid.is_empty(), "GridCraft has no Traditional Chinese");
        let p = Preferences { language: "pt-br".into(), ..Preferences::default() };
        let mut cad = Map::new();
        apply("cadcraft", &p, &[Field::Language], "", Mode::Override, &mut cad);
        assert_eq!(inner(&cad, "cadcraft.prefs")["interfaceLanguage"], "pt-BR");
        apply("gridcraft", &p, &[Field::Language], "", Mode::Override, &mut grid);
        assert_eq!(inner(&grid, "gridcraft.ui")["language"], "pt");
    }

    #[test]
    fn fill_missing_keeps_what_the_app_chose() {
        let mut word = Map::new();
        word.insert("wordcraft.ui".into(), json!(r#"{"theme":"light","language":"fr","author":"Ada","initials":""}"#));
        apply("wordcraft", &prefs(), &Field::ALL, "", Mode::FillMissing, &mut word);
        let ui = inner(&word, "wordcraft.ui");
        assert_eq!((ui["theme"].as_str(), ui["language"].as_str(), ui["author"].as_str()), (Some("light"), Some("fr"), Some("Ada")));
        assert_eq!(ui["initials"], "GH", "empty initials are filled");

        let mut word = Map::new();
        word.insert("wordcraft.ui".into(), json!(r#"{"language":"auto","author":"WordCraft User"}"#));
        apply("wordcraft", &prefs(), &Field::ALL, "", Mode::FillMissing, &mut word);
        let ui = inner(&word, "wordcraft.ui");
        assert_eq!((ui["language"].as_str(), ui["author"].as_str()), (Some("de"), Some("Gabriel Heiler")), "auto and the placeholder count as unset");

        let mut empty = Map::new();
        apply("wordcraft", &Preferences::default(), &Field::ALL, "María López", Mode::FillMissing, &mut empty);
        let ui = inner(&empty, "wordcraft.ui");
        assert_eq!((ui["author"].as_str(), ui["initials"].as_str()), (Some("María López"), Some("ML")), "the account name by default");
        assert!(ui.get("language").is_none(), "auto: the app follows the browser itself");
    }

    #[test]
    fn foreign_or_missing_documents_are_left_alone() {
        let mut cad = Map::new();
        apply("cadcraft", &prefs(), &[Field::Units], "", Mode::Override, &mut cad);
        assert!(cad.is_empty(), "the CADCraft profile is only edited once it exists");
        cad.insert("cadcraft.profile".into(), json!(r#"{"version":1,"sysvars":{"MEASUREINIT":0,"OSMODE":4133}}"#));
        apply("cadcraft", &prefs(), &[Field::Units], "", Mode::Override, &mut cad);
        assert_eq!(inner(&cad, "cadcraft.profile")["sysvars"], json!({"MEASUREINIT": 1, "OSMODE": 4133}));

        let mut odd = Map::new();
        odd.insert("wordcraft.ui".into(), json!("not json"));
        assert!(!apply("wordcraft", &prefs(), &Field::ALL, "", Mode::Override, &mut odd));
        assert_eq!(odd["wordcraft.ui"], "not json");
    }

    #[test]
    fn changes_are_listed() {
        let a = Preferences::default();
        let b = Preferences { theme: "dark".into(), author: "X Y".into(), ..a.clone() };
        assert_eq!(a.changed(&b), vec![Field::Theme, Field::Author, Field::Initials]);
        assert!(a.changed(&a).is_empty());
        assert!(app_takes("wordcraft", Field::Initials) && !app_takes("gridcraft", Field::Initials));
        assert!(!app_takes("lightcraft", Field::Language), "LightCraft keeps its options outside localStorage");
    }

    #[test]
    fn every_listed_app_language_is_in_the_union() {
        for app in ["wordcraft", "pdfcraft", "gridcraft", "cadcraft", "lightcraft"] {
            for l in LANGUAGES {
                let _ = app_language(app, l.code);
            }
        }
        let all: Vec<&str> = WORDCRAFT
            .iter()
            .chain(PDFCRAFT)
            .chain(LIGHTCRAFT)
            .copied()
            .chain(GRIDCRAFT.iter().chain(CADCRAFT).map(|(c, _)| *c))
            .collect();
        for code in all {
            assert!(language_by_code(code).is_some(), "{code} is missing from LANGUAGES");
        }
        let mut codes: Vec<&str> = LANGUAGES.iter().map(|l| l.code).collect();
        let n = codes.len();
        codes.sort();
        codes.dedup();
        assert_eq!(codes.len(), n, "no duplicates");
    }
}
