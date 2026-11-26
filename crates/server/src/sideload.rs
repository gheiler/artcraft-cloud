//! Sideloaded apps: metadata, file-type conflicts, the official blocklist, and source checks
//! (GitHub releases or a https manifest).

use std::collections::BTreeMap;
use std::path::Path;

use artcraft_cloud_types::registry::{compare_versions, is_loopback_http, valid_id, valid_sha256, valid_version, Registry};
use artcraft_cloud_types::{AppSource, TypeConflict};

use crate::manifest::{read_manifest, OfferedVersion, SideloadMeta, SideloadSource, Manifest};
use crate::registry::unix_now;

/// Grants a sideloaded app may receive. Mail and calendar stay official-only.
pub const SIDELOAD_GRANTS: &[&str] = &["document", "drive", "clipboard", "fullscreen"];

pub fn normalize_id(id: &str) -> Result<String, String> {
    let id = id.trim().to_ascii_lowercase();
    if !valid_id(&id) {
        return Err("the app id must be lower-case letters, digits or hyphens (at most 40 characters)".into());
    }
    Ok(id)
}

pub fn normalize_version(version: &str) -> Result<String, String> {
    let version = version.trim();
    let version = if version.is_empty() { "0.0.1" } else { version };
    if !valid_version(version) {
        return Err(format!("bad version {version:?}"));
    }
    Ok(version.to_string())
}

/// Lower-case suffixes without a leading dot, unique, at most 16 of at most 16 characters.
pub fn normalize_extensions(list: &[String]) -> Result<Vec<String>, String> {
    let mut out = Vec::new();
    for raw in list {
        for part in raw.split([',', ' ', ';', '\n', '\t']) {
            let ext = part.trim().trim_start_matches('.').to_ascii_lowercase();
            if ext.is_empty() {
                continue;
            }
            if ext.len() > 16 || !ext.chars().all(|c| c.is_ascii_alphanumeric()) {
                return Err(format!("bad file type {part:?} (use a short suffix, for example docx)"));
            }
            if !out.contains(&ext) {
                out.push(ext);
            }
        }
    }
    if out.len() > 16 {
        return Err("too many file types (16 at most)".into());
    }
    Ok(out)
}

pub fn normalize_grants(list: &[String], bridge: bool) -> Result<Vec<String>, String> {
    let mut out = Vec::new();
    if bridge {
        out.push("document".into());
    }
    for raw in list {
        let grant = raw.trim().to_ascii_lowercase();
        if grant.is_empty() || out.iter().any(|g| g == &grant) {
            continue;
        }
        if grant == "mail" || grant == "calendar" {
            return Err("mail and calendar access cannot be granted to a sideloaded app".into());
        }
        if !SIDELOAD_GRANTS.contains(&grant.as_str()) {
            return Err(format!("unknown grant {raw:?}"));
        }
        if grant == "document" && !bridge {
            return Err("document access needs a host-bridge app".into());
        }
        out.push(grant);
    }
    Ok(out)
}

pub fn normalize_source(source: Option<&AppSource>) -> Result<Option<SideloadSource>, String> {
    let Some(source) = source else {
        return Ok(None);
    };
    let kind = source.kind.trim().to_ascii_lowercase();
    let url = source.url.trim();
    if url.is_empty() {
        return Err("a source needs a GitHub repository or a manifest URL".into());
    }
    match kind.as_str() {
        "github" => {
            let repo = github_repo(url)?;
            Ok(Some(SideloadSource { kind: "github".into(), url: repo }))
        }
        "manifest" => {
            if !url.starts_with("https://") && !is_loopback_http(url) {
                return Err("a manifest source must be an https URL (or a loopback http URL in a lab)".into());
            }
            Ok(Some(SideloadSource { kind: "manifest".into(), url: url.to_string() }))
        }
        _ => Err("a source is either github or manifest".into()),
    }
}

fn github_repo(url: &str) -> Result<String, String> {
    let rest = url
        .trim()
        .trim_end_matches('/')
        .strip_prefix("https://github.com/")
        .or_else(|| url.trim().strip_prefix("http://github.com/"))
        .unwrap_or(url.trim());
    let mut parts = rest.split('/').filter(|p| !p.is_empty());
    let owner = parts.next().unwrap_or("");
    let repo = parts.next().unwrap_or("").trim_end_matches(".git");
    if owner.is_empty() || repo.is_empty()
        || !owner.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
        || !repo.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
    {
        return Err("a GitHub source looks like owner/repo".into());
    }
    Ok(format!("{owner}/{repo}"))
}

/// File types this install would take from another app, unless the admin confirms.
pub fn type_conflicts(manifest: &Manifest, registry: Option<&Registry>, app_id: &str, extensions: &[String], claim_defaults: bool) -> Vec<TypeConflict> {
    let mut out = Vec::new();
    for ext in extensions {
        let ext = ext.to_ascii_lowercase();
        if let Some(conflict) = owner_of(manifest, registry, &ext).filter(|(id, _, _)| id != app_id) {
            let (id, title, official) = conflict;
            // Official types always need confirmation (and claim_defaults to become the handler).
            // Two sideloads need confirmation too, unless this app is already the default.
            if official || manifest.defaults.get(&ext).is_none_or(|owner| owner != app_id) {
                let _ = claim_defaults;
                out.push(TypeConflict { ext, app: id, title, official });
            }
        }
    }
    out
}

fn owner_of(manifest: &Manifest, registry: Option<&Registry>, ext: &str) -> Option<(String, String, bool)> {
    if let Some(id) = manifest.defaults.get(ext) {
        if let Some(title) = title_of(manifest, registry, id) {
            let official = registry.is_some_and(|r| r.app(id).is_some()) && !manifest.apps.get(id).is_some_and(|r| r.is_sideload());
            return Some((id.clone(), title, official));
        }
    }
    if let Some(reg) = registry {
        if let Some(app) = reg.apps.iter().find(|a| a.extensions.iter().any(|e| e.eq_ignore_ascii_case(ext))) {
            return Some((app.id.clone(), app.title.clone(), true));
        }
    }
    for (id, row) in &manifest.apps {
        if let Some(meta) = &row.sideload {
            if meta.extensions.iter().any(|e| e.eq_ignore_ascii_case(ext)) {
                return Some((id.clone(), meta.name.clone(), false));
            }
        }
    }
    None
}

fn title_of(manifest: &Manifest, registry: Option<&Registry>, id: &str) -> Option<String> {
    if let Some(app) = registry.and_then(|r| r.app(id)) {
        return Some(app.title.clone());
    }
    manifest.apps.get(id).and_then(|r| r.sideload.as_ref().map(|m| m.name.clone())).or_else(|| Some(id.to_string()))
}

/// Recompute the default handler for every suffix from official installs, then sideloads.
pub fn rebuild_defaults(manifest: &mut Manifest, registry: Option<&Registry>) {
    let mut defaults = BTreeMap::new();
    if let Some(reg) = registry {
        for app in &reg.apps {
            let installed = manifest.apps.get(&app.id);
            if installed.is_none_or(|row| row.disabled.is_some() || row.is_sideload()) {
                continue;
            }
            for ext in &app.extensions {
                defaults.entry(ext.to_ascii_lowercase()).or_insert_with(|| app.id.clone());
            }
        }
    }
    let mut sideloads: Vec<(String, i64, SideloadMeta)> = manifest
        .apps
        .iter()
        .filter(|(_, row)| row.disabled.is_none())
        .filter_map(|(id, row)| row.sideload.clone().map(|meta| (id.clone(), row.installed_at, meta)))
        .collect();
    sideloads.sort_by(|a, b| a.1.cmp(&b.1).then(a.0.cmp(&b.0)));
    for (id, _, meta) in sideloads {
        for ext in &meta.extensions {
            let ext = ext.to_ascii_lowercase();
            if meta.claim_defaults || !defaults.contains_key(&ext) {
                defaults.insert(ext, id.clone());
            }
        }
    }
    manifest.defaults = defaults;
}

/// The official registry blocks this id+version, or this checksum (even under another name).
pub fn blocked_by_registry(registry: &Registry, app_id: &str, version: &str, sha256: &str) -> Option<String> {
    let sha = sha256.to_ascii_lowercase();
    if let Some(app) = registry.app(app_id) {
        if let artcraft_cloud_types::registry::Verdict::Blocked { reason, .. } = app.verdict(version, crate::registry::cloud_version()) {
            return Some(reason);
        }
        if let Some(found) = app.versions.iter().find(|v| v.sha256.eq_ignore_ascii_case(&sha)) {
            if let artcraft_cloud_types::registry::Verdict::Blocked { reason, .. } = found.verdict(crate::registry::cloud_version()) {
                return Some(reason);
            }
        }
    }
    for app in &registry.apps {
        for ver in &app.versions {
            if ver.sha256.eq_ignore_ascii_case(&sha) {
                if let artcraft_cloud_types::registry::Verdict::Blocked { reason, .. } = ver.verdict(crate::registry::cloud_version()) {
                    return Some(format!("checksum blocked as {} {} ({reason})", app.id, ver.version));
                }
            }
        }
    }
    None
}

/// A version a source offers.
#[derive(Debug, Clone)]
pub struct SourceVersion {
    pub version: String,
    pub url: String,
    pub sha256: String,
    pub signed: String,
}

pub async fn check_source(app_id: &str, source: &SideloadSource) -> Result<Vec<SourceVersion>, String> {
    match source.kind.as_str() {
        "github" => github_versions(app_id, &source.url).await,
        "manifest" => manifest_versions(app_id, &source.url).await,
        kind => Err(format!("unknown source kind {kind}")),
    }
}

pub fn newest(versions: &[SourceVersion]) -> Option<&SourceVersion> {
    versions.iter().max_by(|a, b| compare_versions(&a.version, &b.version))
}

pub fn offered_from(found: &SourceVersion) -> OfferedVersion {
    OfferedVersion {
        version: found.version.clone(),
        url: found.url.clone(),
        sha256: found.sha256.clone(),
        signed: found.signed.clone(),
        checked_at: unix_now(),
    }
}

async fn github_versions(app_id: &str, repo: &str) -> Result<Vec<SourceVersion>, String> {
    let client = reqwest::Client::builder()
        .user_agent(crate::registry::USER_AGENT)
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|err| err.to_string())?;
    let url = format!("https://api.github.com/repos/{repo}/releases?per_page=20");
    let response = client.get(&url).header("accept", "application/vnd.github+json").send().await.map_err(|err| err.to_string())?;
    if !response.status().is_success() {
        return Err(format!("GitHub releases for {repo} returned {}", response.status()));
    }
    let releases: serde_json::Value = response.json().await.map_err(|err| err.to_string())?;
    let Some(list) = releases.as_array() else {
        return Err("GitHub releases response is not a list".into());
    };
    let mut out = Vec::new();
    for release in list {
        if release.get("draft").and_then(|v| v.as_bool()).unwrap_or(false) {
            continue;
        }
        let assets = release.get("assets").and_then(|v| v.as_array()).cloned().unwrap_or_default();
        let zip = assets.iter().find_map(|asset| {
            let name = asset.get("name")?.as_str()?;
            let url = asset.get("browser_download_url")?.as_str()?;
            let version = version_from_asset(app_id, name)?;
            Some((version, name.to_string(), url.to_string()))
        });
        let Some((version, zip_name, zip_url)) = zip else {
            continue;
        };
        let sums = asset_text(&client, &assets, "SHA256SUMS.txt").await?;
        let Some(sums) = sums else {
            continue;
        };
        let sha256 = crate::apps::expected_sha256(&sums, &zip_name)?;
        let signed = if let Some(sig) = asset_text(&client, &assets, "SHA256SUMS.txt.sig").await? {
            if crate::apps::looks_like_ed25519(&sig) {
                "ed25519".into()
            } else {
                "openpgp".into()
            }
        } else {
            "none".into()
        };
        if valid_version(&version) && valid_sha256(&sha256) {
            out.push(SourceVersion { version, url: zip_url, sha256, signed });
        }
    }
    if out.is_empty() {
        return Err(format!("no {app_id}-web-<version>.zip releases with SHA256SUMS.txt on {repo}"));
    }
    Ok(out)
}

fn version_from_asset(app_id: &str, name: &str) -> Option<String> {
    let prefix = format!("{app_id}-web-");
    let rest = name.strip_prefix(&prefix)?.strip_suffix(".zip")?;
    if rest.is_empty() {
        return None;
    }
    Some(rest.to_string())
}

async fn asset_text(client: &reqwest::Client, assets: &[serde_json::Value], name: &str) -> Result<Option<String>, String> {
    let Some(url) = assets.iter().find_map(|asset| {
        (asset.get("name").and_then(|v| v.as_str()) == Some(name)).then(|| asset.get("browser_download_url").and_then(|v| v.as_str()).map(str::to_string))?
    }) else {
        return Ok(None);
    };
    let response = client.get(&url).send().await.map_err(|err| err.to_string())?;
    if !response.status().is_success() {
        return Err(format!("{name} returned {}", response.status()));
    }
    let text = response.text().await.map_err(|err| err.to_string())?;
    Ok(Some(text))
}

async fn manifest_versions(app_id: &str, url: &str) -> Result<Vec<SourceVersion>, String> {
    if !url.starts_with("https://") && !is_loopback_http(url) {
        return Err("a manifest source must be an https URL".into());
    }
    let client = reqwest::Client::builder()
        .user_agent(crate::registry::USER_AGENT)
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|err| err.to_string())?;
    let response = client.get(url).send().await.map_err(|err| err.to_string())?;
    if !response.status().is_success() {
        return Err(format!("manifest returned {}", response.status()));
    }
    let value: serde_json::Value = response.json().await.map_err(|err| format!("manifest is not JSON: {err}"))?;
    let id = value.get("id").and_then(|v| v.as_str()).unwrap_or(app_id);
    if id != app_id {
        return Err(format!("manifest is for {id}, not {app_id}"));
    }
    let list = if let Some(list) = value.get("versions").and_then(|v| v.as_array()) {
        list.clone()
    } else if value.get("version").is_some() {
        vec![value.clone()]
    } else {
        return Err("manifest has no versions".into());
    };
    let mut out = Vec::new();
    for item in list {
        let version = item.get("version").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let zip = item.get("url").or_else(|| item.get("zip")).and_then(|v| v.as_str()).unwrap_or("").to_string();
        let sha256 = item.get("sha256").and_then(|v| v.as_str()).unwrap_or("").to_ascii_lowercase();
        if !valid_version(&version) || !valid_sha256(&sha256) {
            continue;
        }
        if !zip.starts_with("https://") && !is_loopback_http(&zip) {
            continue;
        }
        let signed = item
            .get("signature")
            .or_else(|| item.get("signed"))
            .and_then(|v| v.as_str())
            .map(|s| if s.starts_with("ed25519") || s == "ed25519" { "ed25519" } else { "none" })
            .unwrap_or("none")
            .to_string();
        out.push(SourceVersion { version, url: zip, sha256, signed });
    }
    if out.is_empty() {
        return Err("manifest lists no usable versions".into());
    }
    Ok(out)
}

/// Canonical isolation mode stored on a sideload (`sandbox`, `regular` or `domain`).
pub fn isolation_of(meta: &crate::manifest::SideloadMeta) -> String {
    artcraft_cloud_types::normalize_isolation_name(&meta.isolation)
        .unwrap_or(artcraft_cloud_types::ISOLATION_SANDBOX)
        .to_string()
}

pub fn isolation_of_row(row: &crate::manifest::InstalledApp) -> Option<String> {
    row.sideload.as_ref().map(isolation_of)
}

/// Validate an isolation choice. `regular` needs an extra confirmation; `domain` needs `ARTCRAFT_APPS_URL`.
pub fn normalize_isolation(value: Option<&str>, apps_url: bool, confirm_regular: bool) -> Result<String, String> {
    let raw = value.unwrap_or(artcraft_cloud_types::ISOLATION_SANDBOX);
    let mode = artcraft_cloud_types::normalize_isolation_name(raw).ok_or_else(|| {
        format!("unknown isolation mode ({raw}); use sandbox, regular or domain")
    })?;
    if mode == artcraft_cloud_types::ISOLATION_REGULAR && !confirm_regular {
        return Err("regular isolation lets the app use the signed-in session; confirm that before choosing it".into());
    }
    if mode == artcraft_cloud_types::ISOLATION_DOMAIN && !apps_url {
        return Err("separate-domain isolation needs ARTCRAFT_APPS_URL (a second hostname for this same server, for example https://apps.your-domain)".into());
    }
    Ok(mode.to_string())
}

/// Build sideload metadata from validated fields.
#[allow(clippy::too_many_arguments)]
pub fn meta(name: &str, publisher: &str, blurb: &str, bridge: bool, extensions: Vec<String>, grants: Vec<String>, claim_defaults: bool, source: Option<SideloadSource>, isolation: String) -> Result<SideloadMeta, String> {
    let name = name.trim();
    if name.is_empty() || name.len() > 80 {
        return Err("give the app a name (at most 80 characters)".into());
    }
    let publisher = publisher.trim();
    let blurb = blurb.trim();
    if publisher.len() > 80 {
        return Err("publisher is too long".into());
    }
    if blurb.len() > 240 {
        return Err("description is too long".into());
    }
    if bridge && extensions.is_empty() {
        return Err("a document app needs at least one file type".into());
    }
    Ok(SideloadMeta {
        name: name.to_string(),
        publisher: publisher.to_string(),
        blurb: blurb.to_string(),
        bridge,
        extensions,
        grants,
        claim_defaults,
        source,
        offered: None,
        isolation,
    })
}

pub fn look_up_installed(data: &Path, id: &str) -> Option<crate::manifest::InstalledApp> {
    read_manifest(data).apps.get(id).cloned()
}

/// A short helper so HTTP and CLI can record that official now lists this sideload.
pub fn official_notice(id: &str) -> String {
    format!("The app registry now lists {id}. You can switch this install to the trusted build.")
}
