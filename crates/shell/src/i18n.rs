//! Interface languages of the shell.
//!
//! The text in the code is English and is the lookup key, as in the Crafting Apps (craft-i18n):
//! `t("My files")`. `locales/<code>.tsv` maps it to a translation, one `English<TAB>translation`
//! row per line; a missing row shows the English. `{name}` placeholders are filled by [`tf`] after
//! the lookup, so a translation may move them.
//!
//! The language is chosen once per page load ([`set_language`]); changing it in Settings reloads
//! the page. The apps have their own catalogs and get the user's language through their stored
//! options (`artcraft_cloud_types::prefs`).

use std::cell::{Cell, RefCell};
use std::collections::HashMap;

/// Languages the shell is translated into, besides English.
pub const SHELL_LANGUAGES: &[(&str, &str)] = &[
    ("de", include_str!("../locales/de.tsv")),
    ("es", include_str!("../locales/es.tsv")),
    ("fr", include_str!("../locales/fr.tsv")),
    ("it", include_str!("../locales/it.tsv")),
    ("pt-br", include_str!("../locales/pt-br.tsv")),
];

thread_local! {
    static CODE: Cell<&'static str> = const { Cell::new("en") };
    static TABLE: RefCell<HashMap<&'static str, &'static str>> = RefCell::new(HashMap::new());
}

/// Parse one catalog: `English<TAB>translation` rows; `#` lines and blank lines are skipped.
pub fn parse(catalog: &'static str) -> HashMap<&'static str, &'static str> {
    catalog
        .lines()
        .filter(|line| !line.trim().is_empty() && !line.starts_with('#'))
        .filter_map(|line| line.split_once('\t'))
        .filter(|(_, to)| !to.trim().is_empty())
        .collect()
}

/// The shell language for a preference (`auto` or a code) and the browser's languages:
/// the exact code, else its base language (`pt-pt` → none, `de-at` → `de`), else English.
pub fn resolve(preference: &str, browser: &[String]) -> &'static str {
    let find = |tag: &str| -> Option<&'static str> {
        let tag = tag.trim().to_ascii_lowercase().replace('_', "-");
        if tag == "en" || tag.starts_with("en-") {
            return Some("en");
        }
        if let Some((code, _)) = SHELL_LANGUAGES.iter().find(|(code, _)| *code == tag) {
            return Some(code);
        }
        if tag == "pt-br" || tag.starts_with("pt-br-") {
            return Some("pt-br");
        }
        let base = tag.split('-').next().unwrap_or_default();
        SHELL_LANGUAGES
            .iter()
            .find(|(code, _)| *code == base && !code.contains('-'))
            .map(|(code, _)| *code)
    };
    if preference != "auto" {
        return find(preference).unwrap_or("en");
    }
    browser.iter().find_map(|tag| find(tag)).unwrap_or("en")
}

/// Show the interface in `code` from now on (one of [`SHELL_LANGUAGES`], else English).
pub fn set_language(code: &str) {
    let found = SHELL_LANGUAGES.iter().find(|(c, _)| *c == code);
    CODE.with(|c| c.set(found.map_or("en", |(c, _)| c)));
    TABLE
        .with(|table| *table.borrow_mut() = found.map(|(_, text)| parse(text)).unwrap_or_default());
}

/// The language the interface is shown in.
pub fn language() -> &'static str {
    CODE.with(Cell::get)
}

/// `english` in the interface language.
pub fn t(english: &'static str) -> &'static str {
    TABLE.with(|table| table.borrow().get(english).copied().unwrap_or(english))
}

/// [`t`], then `{name}` placeholders filled in.
pub fn tf(english: &'static str, values: &[(&str, &str)]) -> String {
    let mut out = t(english).to_string();
    for (name, value) in values {
        out = out.replace(&format!("{{{name}}}"), value);
    }
    out
}

/// The browser's preferred languages, most preferred first.
pub fn browser_languages() -> Vec<String> {
    let Some(navigator) = web_sys::window().map(|w| w.navigator()) else {
        return Vec::new();
    };
    let mut out: Vec<String> = navigator
        .languages()
        .iter()
        .filter_map(|v| v.as_string())
        .collect();
    if let Some(one) = navigator.language() {
        out.push(one);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every `t("…")` / `tf("…")` key in the shell's source.
    fn keys() -> Vec<String> {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/src");
        let mut out = Vec::new();
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            let text = std::fs::read_to_string(&path).unwrap();
            let code = text.split("#[cfg(test)]").next().unwrap_or_default();
            // `t("…")` and `tf("…", …)`, also when rustfmt put the string on the next line.
            for start in ["t(", "tf("] {
                for (at, _) in code.match_indices(start) {
                    let before = code[..at].chars().last().unwrap_or(' ');
                    if before.is_alphanumeric() || before == '_' {
                        continue;
                    }
                    let Some(rest) = code[at + start.len()..].trim_start().strip_prefix('"') else {
                        continue;
                    };
                    if let Some(end) = rest.find('"') {
                        out.push(rest[..end].replace("\\\"", "\""));
                    }
                }
            }
        }
        out.sort();
        out.dedup();
        out
    }

    #[test]
    fn every_string_is_translated_and_keeps_its_placeholders() {
        let keys = keys();
        assert!(keys.len() > 150, "found {} keys", keys.len());
        for (code, catalog) in SHELL_LANGUAGES {
            let table = parse(catalog);
            let missing: Vec<&String> = keys
                .iter()
                .filter(|k| !table.contains_key(k.as_str()))
                .collect();
            assert!(missing.is_empty(), "{code} lacks {missing:?}");
            let unused: Vec<&&str> = table
                .keys()
                .filter(|k| !keys.iter().any(|key| key == **k))
                .collect();
            assert!(
                unused.is_empty(),
                "{code} has rows no code uses: {unused:?}"
            );
            for (from, to) in &table {
                let holes = |s: &str| {
                    let mut v: Vec<String> = s
                        .split('{')
                        .skip(1)
                        .filter_map(|p| p.split_once('}'))
                        .map(|(n, _)| n.to_string())
                        .collect();
                    v.sort();
                    v
                };
                assert_eq!(holes(from), holes(to), "{code}: placeholders of {from:?}");
            }
            let mut rows: Vec<&str> = catalog
                .lines()
                .filter_map(|l| l.split_once('\t'))
                .map(|(k, _)| k)
                .collect();
            let n = rows.len();
            rows.sort();
            rows.dedup();
            assert_eq!(rows.len(), n, "{code} has a duplicate row");
        }
    }

    #[test]
    fn languages_resolve_with_english_fallback() {
        let browser = |tags: &[&str]| tags.iter().map(|t| t.to_string()).collect::<Vec<_>>();
        assert_eq!(resolve("auto", &browser(&["de-AT", "en"])), "de");
        assert_eq!(resolve("auto", &browser(&["ja", "fr-CA"])), "fr");
        assert_eq!(resolve("auto", &browser(&["ja"])), "en");
        assert_eq!(resolve("pt-br", &[]), "pt-br");
        assert_eq!(resolve("pt-br", &browser(&["de"])), "pt-br");
        assert_eq!(
            resolve("ja", &browser(&["de"])),
            "en",
            "a chosen language the shell lacks shows English"
        );
        assert_eq!(resolve("es", &[]), "es");
        set_language("de");
        assert_eq!(language(), "de");
        assert_eq!(t("My files"), "Meine Dateien");
        assert_eq!(tf("Share {name}", &[("name", "a.docx")]), "a.docx teilen");
        assert_eq!(t("no such text"), "no such text");
        set_language("xx");
        assert_eq!((language(), t("My files")), ("en", "My files"));
    }
}
