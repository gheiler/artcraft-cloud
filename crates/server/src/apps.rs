//! Install, update, roll back and remove the Crafting Apps' web builds in the data volume, as the
//! signed app registry allows (`registry.rs`). Used by the server (the Apps page, the start-up and
//! periodic checks) and by the command line.
//!
//! Every download is checked three ways where it can be: the registry's SHA-256 for the archive,
//! the app's own release signature over `SHA256SUMS.txt` (ed25519 or OpenPGP, keys pinned in the
//! registry), and the archive's contents (safe names, the host bridge).

use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::{Component, Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use artcraft_cloud_types::registry::{compare_versions, Registry, RegistryApp, RegistryVersion, Verdict};
use fs2::FileExt;
use sha2::{Digest, Sha256};
use zip::ZipArchive;

use crate::manifest::{read_manifest, version_dir, write_manifest, InstalledApp, SideloadMeta};
use crate::sideload;
use crate::registry::{cloud_version, Loaded, USER_AGENT};

const MAX_UNPACKED: u64 = 800 * 1024 * 1024;

/// Reports the phase (`downloading`, `verifying`, `installing`) and a percentage when known.
pub type Progress = Arc<dyn Fn(&str, Option<u8>) + Send + Sync>;

pub fn no_progress() -> Progress {
    Arc::new(|_, _| {})
}

/// What to do when the registry blocks an installed version (`ARTCRAFT_BLOCKED_APPS`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockAction {
    /// Warn only.
    Warn,
    /// Roll back or disable for security blocks; warn for the others (the default).
    Security,
    /// Roll back or disable for every block.
    All,
}

impl BlockAction {
    pub fn from_env() -> Self {
        match std::env::var("ARTCRAFT_BLOCKED_APPS").ok().as_deref().map(str::trim) {
            Some("warn") => BlockAction::Warn,
            Some("all") => BlockAction::All,
            _ => BlockAction::Security,
        }
    }
}

/// Which version an install means: the one asked for, else the recommended (or newest allowed).
pub fn choose<'a>(app: &'a RegistryApp, version: Option<&str>) -> Result<&'a RegistryVersion, String> {
    let cloud = cloud_version();
    let v = match version {
        Some(want) => app.version(want.trim_start_matches('v')).ok_or_else(|| format!("{} {want} is not in the registry", app.id))?,
        None => app.target(cloud).ok_or_else(|| format!("the registry allows no version of {} for artcraftCloud {cloud}", app.id))?,
    };
    match v.verdict(cloud) {
        Verdict::Allowed => Ok(v),
        Verdict::Blocked { reason, .. } => Err(format!("{} {} is blocked by the registry: {reason}", app.id, v.version)),
        Verdict::Incompatible { reason } => Err(format!("{} {} is not compatible with artcraftCloud {cloud}: {reason}", app.id, v.version)),
        Verdict::Unlisted => Err(format!("{} {} is not in the registry", app.id, v.version)),
    }
}

/// Download, verify and install one version the registry allows. Returns the version installed
/// and how it was verified; an install of what is already there does nothing.
pub async fn install(data: &Path, loaded: &Loaded, app_id: &str, version: Option<&str>, progress: Progress) -> Result<(String, String), String> {
    let registry = &loaded.registry;
    if registry.expired(crate::registry::unix_now()) {
        return Err("the app registry has expired; refresh it (Apps › Refresh) before installing".into());
    }
    let app = registry.app(app_id).ok_or_else(|| format!("{app_id} is not in the app registry"))?;
    let v = choose(app, version)?;
    let manifest = read_manifest(data);
    if let Some(row) = manifest.apps.get(app_id) {
        if row.version == v.version
            && version_dir(data, app_id, &v.version).join("index.html").is_file()
            && row.disabled.is_none()
            && row.sideload.is_none()
            && row.sha256.eq_ignore_ascii_case(&v.sha256)
        {
            return Ok((v.version.clone(), row.signature.clone()));
        }
    }
    let client = reqwest::Client::builder().user_agent(USER_AGENT).build().map_err(|err| err.to_string())?;
    let scratch = data.join("tmp").join(format!("fetch-{app_id}-{}-{}", v.version, std::process::id()));
    let _ = fs::remove_dir_all(&scratch);
    fs::create_dir_all(&scratch).map_err(|err| err.to_string())?;
    let result = install_in(&client, data, app, v, &scratch, progress).await;
    let _ = fs::remove_dir_all(&scratch);
    if result.is_ok() {
        refresh_defaults(data, Some(registry));
    }
    result.map(|signature| (v.version.clone(), signature))
}

async fn install_in(client: &reqwest::Client, data: &Path, app: &RegistryApp, v: &RegistryVersion, scratch: &Path, progress: Progress) -> Result<String, String> {
    // The release signature first: it is small, and a bad one stops before the large download.
    progress("verifying", None);
    let signature = verify_release(client, app, v, scratch).await?;
    let zip_path = scratch.join(&v.asset);
    let cache = app_cache();
    let cached = cache.as_ref().map(|dir| dir.join(&v.asset)).filter(|path| file_sha256(path).is_ok_and(|sha| sha.eq_ignore_ascii_case(&v.sha256)));
    match &cached {
        Some(path) => {
            fs::copy(path, &zip_path).map_err(|err| format!("copy {}: {err}", path.display()))?;
            tracing::info!(app = app.id, version = v.version, "web build from the app cache");
        }
        None => {
            progress("downloading", Some(0));
            download(client, &app.download_url(v), &zip_path, Some(&progress)).await?;
        }
    }
    let actual = file_sha256(&zip_path)?;
    if !actual.eq_ignore_ascii_case(&v.sha256) {
        return Err(format!("{} sha256 {actual} is not the registry's {}", v.asset, v.sha256));
    }
    progress("installing", None);
    let (data2, id, version, bridge, zip2, sig2) = (data.to_path_buf(), app.id.clone(), v.version.clone(), app.bridge, zip_path.clone(), signature.clone());
    tokio::task::spawn_blocking(move || install_zip_ex(&data2, &id, bridge, &version, &zip2, &actual, &sig2, None, false)).await.map_err(|err| err.to_string())??;
    if let (Some(dir), None) = (&cache, &cached) {
        if let Err(err) = store_in_cache(dir, &app.id, &v.asset, &zip_path) {
            tracing::warn!(app = app.id, "app cache: {err}");
        }
    }
    tracing::info!(app = app.id, version = v.version, signature, "installed");
    Ok(signature)
}

/// Check the app's own release signature over `SHA256SUMS.txt`, and that the list names the same
/// archive hash as the registry. Returns `verified`, or `registry` for an unsigned release (the
/// registry's hash is then the only check).
async fn verify_release(client: &reqwest::Client, app: &RegistryApp, v: &RegistryVersion, scratch: &Path) -> Result<String, String> {
    if v.signed == "none" {
        return Ok("registry".into());
    }
    let sums_path = scratch.join("SHA256SUMS.txt");
    download(client, &app.release_file_url(v, "SHA256SUMS.txt"), &sums_path, None).await?;
    let sums = fs::read_to_string(&sums_path).map_err(|err| err.to_string())?;
    match v.signed.as_str() {
        "ed25519" => {
            let sig_path = scratch.join("SHA256SUMS.txt.sig");
            download(client, &app.release_file_url(v, "SHA256SUMS.txt.sig"), &sig_path, None).await?;
            let sig = fs::read_to_string(&sig_path).map_err(|err| err.to_string())?;
            verify_ed25519(&app.keys.ed25519, sums.as_bytes(), &sig)?;
        }
        "openpgp" => {
            let sig_path = scratch.join("SHA256SUMS.txt.asc");
            let key_path = scratch.join("release-signing-key.asc");
            download(client, &app.release_file_url(v, "SHA256SUMS.txt.asc"), &sig_path, None).await?;
            download(client, &app.release_file_url(v, "release-signing-key.asc"), &key_path, None).await?;
            let fingerprints = app.keys.openpgp.clone();
            let (k, m, s) = (key_path.clone(), sums_path.clone(), sig_path.clone());
            tokio::task::spawn_blocking(move || verify_openpgp(&k, &m, &s, &fingerprints)).await.map_err(|err| err.to_string())??;
        }
        other => return Err(format!("unknown signature kind {other}")),
    }
    let listed = expected_sha256(&sums, &v.asset)?;
    if !listed.eq_ignore_ascii_case(&v.sha256) {
        return Err(format!("the signed SHA256SUMS.txt names {listed} for {}, the registry {}", v.asset, v.sha256));
    }
    Ok("verified".into())
}

/// Install the registry's default apps that are missing (a new data volume), except those an
/// admin removed.
pub async fn sync_missing(data: &Path, loaded: &Loaded) -> Result<(), String> {
    let manifest = read_manifest(data);
    let mut errors = Vec::new();
    for app in loaded.registry.apps.iter().filter(|a| a.default_install) {
        let present = manifest.apps.get(&app.id).is_some_and(|row| version_dir(data, &app.id, &row.version).join("index.html").is_file());
        if present || manifest.removed.contains(&app.id) {
            continue;
        }
        tracing::info!(app = app.id, "installing web build");
        if let Err(err) = install(data, loaded, &app.id, None, no_progress()).await {
            tracing::error!(app = app.id, "{err}");
            errors.push(format!("{}: {err}", app.id));
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.join("; "))
    }
}

/// Update the apps whose auto-update is on to the version the registry points to.
pub async fn auto_update(data: &Path, loaded: &Loaded) -> Vec<String> {
    let mut log = Vec::new();
    for (id, row) in read_manifest(data).apps {
        if !row.auto_update || row.disabled.is_some() {
            continue;
        }
        if row.is_sideload() {
            if loaded.registry.app(&id).is_some() {
                // Official wins: do not keep pulling a source once the registry lists this id.
                continue;
            }
            match update_from_source(data, &id, None, Some(&loaded.registry), no_progress()).await {
                Ok((version, _)) => log.push(format!("{id} updated to {version} from its source")),
                Err(err) if err.contains("already") || err.contains("no newer") => {}
                Err(err) => log.push(format!("{id}: {err}")),
            }
            continue;
        }
        let Some(app) = loaded.registry.app(&id) else { continue };
        let Some(target) = app.target(cloud_version()) else { continue };
        if compare_versions(&target.version, &row.version) != std::cmp::Ordering::Greater {
            continue;
        }
        match install(data, loaded, &id, Some(&target.version), no_progress()).await {
            Ok((version, _)) => log.push(format!("{id} updated to {version}")),
            Err(err) => log.push(format!("{id}: {err}")),
        }
    }
    log
}

/// The newest version an update would install, when it is newer than `installed`.
pub fn update_for(app: &RegistryApp, installed: &str) -> Option<String> {
    let target = app.target(cloud_version())?;
    (compare_versions(&target.version, installed) == std::cmp::Ordering::Greater).then(|| target.version.clone())
}

/// Act on installed versions the registry now blocks: roll back to the previous version when it is
/// on disk and allowed, else disable the app. Returns what was done.
pub fn enforce(data: &Path, registry: &Registry, action: BlockAction) -> Vec<String> {
    let cloud = cloud_version();
    let mut done = Vec::new();
    let result = with_lock(data, || {
        let mut manifest = read_manifest(data);
        let mut changed = false;
        for (id, row) in manifest.apps.iter_mut() {
            if row.is_sideload() && registry.app(id).is_some() && row.notice.is_none() {
                row.notice = Some(sideload::official_notice(id));
                changed = true;
            }
            let blocked = match registry.app(id).map(|app| app.verdict(&row.version, cloud)) {
                Some(Verdict::Blocked { reason, security }) => Some((reason, security)),
                _ => sideload::blocked_by_registry(registry, id, &row.version, &row.sha256).map(|reason| (reason, true)),
            };
            let Some((reason, security)) = blocked else { continue };
            let act = match action {
                BlockAction::Warn => false,
                BlockAction::Security => security,
                BlockAction::All => true,
            };
            if !act || row.disabled.is_some() {
                continue;
            }
            let previous_ok = row.previous.as_deref().filter(|prev| registry.app(id).is_some_and(|app| app.verdict(prev, cloud).allowed()) && version_dir(data, id, prev).join("index.html").is_file()).map(String::from);
            let blocked = row.version.clone();
            match previous_ok {
                Some(prev) => {
                    row.previous = Some(blocked.clone());
                    row.version = prev.clone();
                    row.notice = Some(format!("Rolled back automatically from {blocked} to {prev}: {blocked} is blocked ({reason})."));
                    done.push(format!("{id}: rolled back from {blocked} to {prev} ({reason})"));
                }
                None => {
                    row.disabled = Some(format!("{blocked} is blocked: {reason}"));
                    row.notice = Some(format!("Disabled automatically: {blocked} is blocked ({reason}) and no allowed earlier version is on this server."));
                    done.push(format!("{id}: disabled, {blocked} is blocked ({reason})"));
                }
            }
            changed = true;
        }
        if changed {
            sideload::rebuild_defaults(&mut manifest, Some(registry));
            write_manifest(data, &manifest)?;
        }
        Ok(())
    });
    if let Err(err) = result {
        done.push(format!("could not apply the registry's blocks: {err}"));
    }
    for line in &done {
        tracing::warn!("{line}");
    }
    done
}

/// Switch back to the version kept from the previous update, unless the registry blocks it.
pub fn rollback(data: &Path, registry: Option<&Registry>, app_id: &str) -> Result<String, String> {
    with_lock(data, || {
        let mut manifest = read_manifest(data);
        let row = manifest.apps.get_mut(app_id).ok_or_else(|| format!("{app_id} is not installed"))?;
        let previous = row.previous.clone().ok_or_else(|| format!("{app_id} has no previous version"))?;
        if !version_dir(data, app_id, &previous).join("index.html").is_file() {
            return Err(format!("previous files for {app_id} {previous} are missing"));
        }
        if let Some(app) = registry.and_then(|r| r.app(app_id)) {
            match app.verdict(&previous, cloud_version()) {
                Verdict::Blocked { reason, .. } => return Err(format!("{app_id} {previous} is blocked by the registry: {reason}")),
                Verdict::Incompatible { reason } => return Err(format!("{app_id} {previous} is not compatible: {reason}")),
                _ => {}
            }
        }
        let current = row.version.clone();
        row.version = previous.clone();
        row.previous = Some(current);
        row.disabled = None;
        row.notice = None;
        write_manifest(data, &manifest)?;
        Ok(previous)
    })
}

/// Delete an app's files and forget it; the start-up check does not bring it back.
pub fn remove(data: &Path, app_id: &str) -> Result<(), String> {
    if !artcraft_cloud_types::registry::valid_id(app_id) {
        return Err(format!("bad app id {app_id:?}"));
    }
    with_lock(data, || {
        let mut manifest = read_manifest(data);
        manifest.apps.remove(app_id);
        if !manifest.removed.iter().any(|id| id == app_id) {
            manifest.removed.push(app_id.to_string());
        }
        write_manifest(data, &manifest)?;
        let dir = data.join("apps").join(app_id);
        if dir.exists() {
            fs::remove_dir_all(&dir).map_err(|err| err.to_string())?;
        }
        Ok(())
    })
}

/// Turn auto-update or the app itself on or off; clear a notice that was read.
pub fn set_settings(data: &Path, app_id: &str, auto_update: Option<bool>, disabled: Option<bool>, dismiss: bool) -> Result<(), String> {
    with_lock(data, || {
        let mut manifest = read_manifest(data);
        let row = manifest.apps.get_mut(app_id).ok_or_else(|| format!("{app_id} is not installed"))?;
        if let Some(on) = auto_update {
            row.auto_update = on;
        }
        match disabled {
            Some(true) => row.disabled = Some("Disabled by the administrator.".into()),
            Some(false) => row.disabled = None,
            None => {}
        }
        if dismiss {
            row.notice = None;
        }
        write_manifest(data, &manifest)
    })
}

/// `apps install`: a local archive, trusted because whoever runs the command put it there. The
/// archive is checked like a download (safe names, the host bridge) but carries no signature.
pub fn install_local(data: &Path, app_id: &str, bridge: bool, version: &str, zip_path: &Path) -> Result<(), String> {
    if !artcraft_cloud_types::registry::valid_version(version) {
        return Err(format!("bad version {version:?}"));
    }
    let sha256 = file_sha256(zip_path)?;
    install_zip(data, app_id, bridge, version, zip_path, &sha256, "local")?;
    println!("{app_id} {version} installed from {}", zip_path.display());
    Ok(())
}

pub fn install_zip(data: &Path, app_id: &str, bridge: bool, version: &str, zip_path: &Path, sha256: &str, signature: &str) -> Result<(), String> {
    install_zip_ex(data, app_id, bridge, version, zip_path, sha256, signature, None, true)
}

/// `sideload` replaces any previous sideload metadata; `keep_sideload` keeps it when `sideload` is
/// none (a source update). Official registry installs pass `keep_sideload = false`.
#[allow(clippy::too_many_arguments)]
pub fn install_zip_ex(
    data: &Path,
    app_id: &str,
    bridge: bool,
    version: &str,
    zip_path: &Path,
    sha256: &str,
    signature: &str,
    sideload: Option<SideloadMeta>,
    keep_sideload: bool,
) -> Result<(), String> {
    if !artcraft_cloud_types::registry::valid_id(app_id) || !artcraft_cloud_types::registry::valid_version(version) {
        return Err(format!("bad app id or version {app_id} {version}"));
    }
    let actual = file_sha256(zip_path)?;
    if !actual.eq_ignore_ascii_case(sha256) {
        return Err(format!("sha256 mismatch for {app_id}"));
    }
    if bridge {
        archive_has_bridge(zip_path, app_id)?;
    } else {
        archive_names_are_safe(zip_path)?;
    }
    let size = fs::metadata(zip_path).map(|m| m.len()).unwrap_or(0);
    with_lock(data, || {
        let staging = data.join("apps").join(format!(".incoming-{app_id}-{version}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&staging);
        fs::create_dir_all(&staging).map_err(|err| err.to_string())?;
        extract_zip(zip_path, &staging)?;
        hoist_index(&staging)?;
        precompress_tree(&staging)?;
        let final_dir = version_dir(data, app_id, version);
        if final_dir.exists() {
            fs::remove_dir_all(&final_dir).map_err(|err| err.to_string())?;
        }
        if let Some(parent) = final_dir.parent() {
            fs::create_dir_all(parent).map_err(|err| err.to_string())?;
        }
        fs::rename(&staging, &final_dir).map_err(|err| err.to_string())?;
        let mut manifest = read_manifest(data);
        let old = manifest.apps.get(app_id).cloned();
        let previous = old.as_ref().and_then(|row| {
            if row.version == version {
                row.previous.clone()
            } else {
                if let Some(old_previous) = &row.previous {
                    if old_previous != version && old_previous != &row.version {
                        let _ = fs::remove_dir_all(version_dir(data, app_id, old_previous));
                    }
                }
                Some(row.version.clone())
            }
        });
        let sideload = sideload.or_else(|| if keep_sideload { old.as_ref().and_then(|row| row.sideload.clone()) } else { None });
        manifest.removed.retain(|id| id != app_id);
        manifest.apps.insert(
            app_id.to_string(),
            InstalledApp {
                version: version.to_string(),
                previous,
                sha256: actual,
                signature: signature.to_string(),
                installed_at: unix_now(),
                disabled: None,
                auto_update: old.as_ref().is_some_and(|row| row.auto_update),
                notice: None,
                sideload,
                size,
            },
        );
        sideload::rebuild_defaults(&mut manifest, None);
        write_manifest(data, &manifest)
    })
}

/// A Danger-zone zip already on disk, with metadata the admin typed.
pub fn install_sideload(data: &Path, app_id: &str, version: &str, zip_path: &Path, meta: SideloadMeta, registry: Option<&Registry>) -> Result<String, String> {
    let sha256 = file_sha256(zip_path)?;
    if let Some(registry) = registry {
        if let Some(reason) = sideload::blocked_by_registry(registry, app_id, version, &sha256) {
            return Err(format!("{app_id} {version} is blocked by the app registry: {reason}"));
        }
    }
    install_zip_ex(data, app_id, meta.bridge, version, zip_path, &sha256, "local", Some(meta), false)?;
    if let Some(registry) = registry {
        refresh_defaults(data, Some(registry));
    }
    Ok(sha256)
}

/// Recompute default file-type handlers from the registry and the sideloads on disk.
pub fn refresh_defaults(data: &Path, registry: Option<&Registry>) {
    let _ = with_lock(data, || {
        let mut manifest = read_manifest(data);
        sideload::rebuild_defaults(&mut manifest, registry);
        write_manifest(data, &manifest)
    });
}

/// Pull a newer (or chosen) version from a connected source and install it.
pub async fn update_from_source(data: &Path, app_id: &str, version: Option<&str>, registry: Option<&Registry>, progress: Progress) -> Result<(String, String), String> {
    let row = read_manifest(data).apps.get(app_id).cloned().ok_or_else(|| format!("{app_id} is not installed"))?;
    let meta = row.sideload.clone().ok_or_else(|| format!("{app_id} is not a sideloaded app"))?;
    let source = meta.source.clone().ok_or_else(|| format!("{app_id} has no source to check"))?;
    progress("verifying", None);
    let found = sideload::check_source(app_id, &source).await?;
    let target = match version {
        Some(want) => found.iter().find(|v| v.version == want).ok_or_else(|| format!("{app_id} {want} is not on the source"))?,
        None => sideload::newest(&found).ok_or_else(|| format!("{app_id} source listed no versions"))?,
    };
    if compare_versions(&target.version, &row.version) != std::cmp::Ordering::Greater && version.is_none() {
        record_offered(data, app_id, sideload::offered_from(target))?;
        return Err(format!("{app_id} has no newer version on its source (latest is {})", target.version));
    }
    if target.version == row.version && target.sha256.eq_ignore_ascii_case(&row.sha256) {
        record_offered(data, app_id, sideload::offered_from(target))?;
        return Err(format!("{app_id} {} is already installed", target.version));
    }
    if let Some(registry) = registry {
        if let Some(reason) = sideload::blocked_by_registry(registry, app_id, &target.version, &target.sha256) {
            return Err(format!("{app_id} {} is blocked by the app registry: {reason}", target.version));
        }
    }
    let client = reqwest::Client::builder().user_agent(USER_AGENT).build().map_err(|err| err.to_string())?;
    let scratch = data.join("tmp").join(format!("sideload-{app_id}-{}-{}", target.version, std::process::id()));
    let _ = fs::remove_dir_all(&scratch);
    fs::create_dir_all(&scratch).map_err(|err| err.to_string())?;
    let zip_path = scratch.join(format!("{app_id}-web-{}.zip", target.version));
    progress("downloading", Some(0));
    let result = download(&client, &target.url, &zip_path, Some(&progress)).await;
    if result.is_ok() {
        progress("installing", None);
        let (data2, id, ver, zip2, _sha, meta2) = (data.to_path_buf(), app_id.to_string(), target.version.clone(), zip_path.clone(), target.sha256.clone(), meta.clone());
        let inst = tokio::task::spawn_blocking(move || install_sideload(&data2, &id, &ver, &zip2, meta2, None)).await.map_err(|err| err.to_string())?;
        if let Err(err) = inst {
            let _ = fs::remove_dir_all(&scratch);
            return Err(err);
        }
        // Re-pin the source and offered version (install_sideload writes the meta it was given).
        let offered = sideload::offered_from(target);
        let _ = record_offered(data, app_id, offered);
        refresh_defaults(data, registry);
    }
    let _ = fs::remove_dir_all(&scratch);
    result?;
    Ok((target.version.clone(), target.sha256.clone()))
}

/// Record the last source check, even when no update is installed.
pub fn record_offered(data: &Path, app_id: &str, offered: crate::manifest::OfferedVersion) -> Result<(), String> {
    with_lock(data, || {
        let mut manifest = read_manifest(data);
        let row = manifest.apps.get_mut(app_id).ok_or_else(|| format!("{app_id} is not installed"))?;
        if let Some(meta) = row.sideload.as_mut() {
            meta.offered = Some(offered);
        }
        write_manifest(data, &manifest)
    })
}

pub fn set_sideload_source(data: &Path, app_id: &str, source: Option<crate::manifest::SideloadSource>) -> Result<(), String> {
    with_lock(data, || {
        let mut manifest = read_manifest(data);
        let row = manifest.apps.get_mut(app_id).ok_or_else(|| format!("{app_id} is not installed"))?;
        let meta = row.sideload.as_mut().ok_or_else(|| format!("{app_id} is not a sideloaded app"))?;
        meta.source = source;
        meta.offered = None;
        write_manifest(data, &manifest)
    })
}

pub fn set_sideload_meta(
    data: &Path,
    app_id: &str,
    extensions: Option<Vec<String>>,
    grants: Option<Vec<String>>,
    claim_defaults: Option<bool>,
    isolation: Option<String>,
    registry: Option<&Registry>,
) -> Result<(), String> {
    with_lock(data, || {
        let mut manifest = read_manifest(data);
        let row = manifest.apps.get_mut(app_id).ok_or_else(|| format!("{app_id} is not installed"))?;
        let meta = row.sideload.as_mut().ok_or_else(|| format!("{app_id} is not a sideloaded app"))?;
        if let Some(ext) = extensions {
            meta.extensions = ext;
        }
        if let Some(grants) = grants {
            meta.grants = grants;
        }
        if let Some(claim) = claim_defaults {
            meta.claim_defaults = claim;
        }
        if let Some(isolation) = isolation {
            meta.isolation = isolation;
        }
        sideload::rebuild_defaults(&mut manifest, registry);
        write_manifest(data, &manifest)
    })
}

pub fn looks_like_ed25519(sig: &str) -> bool {
    use base64::Engine;
    let raw = base64::engine::general_purpose::STANDARD.decode(sig.trim()).ok();
    raw.is_some_and(|b| b.len() == 64)
}

/// `ARTCRAFT_APP_CACHE`: a directory of downloaded web build archives, shared by data volumes
/// (a fresh volume, a second install on the same host, CI). Archives are used only when their
/// SHA-256 matches the release's signed checksum list, so the cache needs no trust of its own.
fn app_cache() -> Option<PathBuf> {
    let dir = std::env::var_os("ARTCRAFT_APP_CACHE").filter(|v| !v.is_empty()).map(PathBuf::from)?;
    fs::create_dir_all(&dir).ok()?;
    Some(dir)
}

/// Keep the new archive and the newest older one of the same app (for a rollback on a new volume).
fn store_in_cache(dir: &Path, app_id: &str, zip_name: &str, zip_path: &Path) -> Result<(), String> {
    let partial = dir.join(format!(".{zip_name}.part"));
    fs::copy(zip_path, &partial).map_err(|err| err.to_string())?;
    fs::rename(&partial, dir.join(zip_name)).map_err(|err| err.to_string())?;
    let prefix = format!("{app_id}-web-");
    let mut older: Vec<(std::time::SystemTime, PathBuf)> = fs::read_dir(dir)
        .map_err(|err| err.to_string())?
        .filter_map(Result::ok)
        .filter(|entry| entry.file_name().to_string_lossy().starts_with(&prefix) && entry.file_name() != zip_name)
        .filter_map(|entry| Some((entry.metadata().ok()?.modified().ok()?, entry.path())))
        .collect();
    older.sort();
    for (_, path) in older.iter().rev().skip(1) {
        let _ = fs::remove_file(path);
    }
    Ok(())
}

pub fn expected_sha256(sums: &str, filename: &str) -> Result<String, String> {
    for line in sums.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut parts = line.split_whitespace();
        let hash = parts.next().unwrap_or("");
        let name = parts.next().unwrap_or("").trim_start_matches('*');
        let base = name.rsplit(['/', '\\']).next().unwrap_or(name);
        if base == filename && hash.len() == 64 && hash.chars().all(|c| c.is_ascii_hexdigit()) {
            return Ok(hash.to_ascii_lowercase());
        }
    }
    Err(format!("SHA256SUMS.txt has no line for {filename}"))
}

fn archive_has_bridge(zip_path: &Path, app_id: &str) -> Result<(), String> {
    let needles = [format!("{app_id}:ready").into_bytes(), b"host=parent".to_vec()];
    let mut archive = ZipArchive::new(File::open(zip_path).map_err(|err| err.to_string())?).map_err(|err| err.to_string())?;
    for index in 0..archive.len() {
        let mut file = archive.by_index(index).map_err(|err| err.to_string())?;
        let name = file.name().to_string();
        check_zip_name(&name)?;
        if file.is_dir() {
            continue;
        }
        if file_contains(&mut file, &needles)? {
            return Ok(());
        }
    }
    Err(format!("{app_id} web build does not contain the host bridge"))
}

fn archive_names_are_safe(zip_path: &Path) -> Result<(), String> {
    let archive = ZipArchive::new(File::open(zip_path).map_err(|err| err.to_string())?).map_err(|err| err.to_string())?;
    for name in archive.file_names() {
        check_zip_name(name)?;
    }
    Ok(())
}

/// Check `SHA256SUMS.txt` against an ed25519 signature in base64 (MailCraft's `cargo xtask
/// release-key sign`) by any of the app's published keys.
pub fn verify_ed25519(keys: &[String], message: &[u8], signature_b64: &str) -> Result<(), String> {
    use base64::Engine;
    use ed25519_dalek::{Signature, VerifyingKey};
    let engine = base64::engine::general_purpose::STANDARD;
    let raw = engine.decode(signature_b64.trim()).map_err(|_| "SHA256SUMS.txt.sig is not base64".to_string())?;
    let signature = Signature::from_slice(&raw).map_err(|_| "SHA256SUMS.txt.sig is not an ed25519 signature".to_string())?;
    for key in keys {
        let Ok(bytes) = engine.decode(key) else { continue };
        let Ok(bytes) = <[u8; 32]>::try_from(bytes.as_slice()) else { continue };
        let Ok(key) = VerifyingKey::from_bytes(&bytes) else { continue };
        if key.verify_strict(message, &signature).is_ok() {
            return Ok(());
        }
    }
    Err("SHA256SUMS.txt.sig matches none of the app's release keys".into())
}

fn extract_zip(zip_path: &Path, dest: &Path) -> Result<(), String> {
    let mut archive = ZipArchive::new(File::open(zip_path).map_err(|err| err.to_string())?).map_err(|err| err.to_string())?;
    let mut total = 0u64;
    for index in 0..archive.len() {
        let mut file = archive.by_index(index).map_err(|err| err.to_string())?;
        let name = file.name().to_string();
        let relative = check_zip_name(&name)?;
        let target = dest.join(&relative);
        if !target.starts_with(dest) {
            return Err(format!("zip path escapes: {name}"));
        }
        if file.is_dir() {
            fs::create_dir_all(&target).map_err(|err| err.to_string())?;
            continue;
        }
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).map_err(|err| err.to_string())?;
        }
        let mut output = File::create(&target).map_err(|err| err.to_string())?;
        let copied = io::copy(&mut file, &mut output).map_err(|err| err.to_string())?;
        total = total.saturating_add(copied);
        if total > MAX_UNPACKED {
            return Err("unpacked app is larger than 800 MiB".into());
        }
    }
    Ok(())
}

fn check_zip_name(name: &str) -> Result<PathBuf, String> {
    if name.contains('\0') || name.starts_with('/') || name.starts_with('\\') {
        return Err(format!("unsafe zip path {name}"));
    }
    let mut out = PathBuf::new();
    for component in Path::new(name).components() {
        match component {
            Component::Normal(part) => out.push(part),
            Component::CurDir => {}
            _ => return Err(format!("unsafe zip path {name}")),
        }
    }
    Ok(out)
}

fn hoist_index(dir: &Path) -> Result<(), String> {
    if dir.join("index.html").is_file() {
        return Ok(());
    }
    let children: Vec<_> = fs::read_dir(dir).map_err(|err| err.to_string())?.filter_map(|e| e.ok()).collect();
    if children.len() == 1 && children[0].path().is_dir() && children[0].path().join("index.html").is_file() {
        let nested = children[0].path();
        for child in fs::read_dir(&nested).map_err(|err| err.to_string())? {
            let child = child.map_err(|err| err.to_string())?;
            fs::rename(child.path(), dir.join(child.file_name())).map_err(|err| err.to_string())?;
        }
        fs::remove_dir_all(&nested).map_err(|err| err.to_string())?;
    }
    if dir.join("index.html").is_file() {
        Ok(())
    } else {
        Err("web build has no index.html".into())
    }
}

fn precompress_tree(root: &Path) -> Result<(), String> {
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(&dir).map_err(|err| err.to_string())? {
            let entry = entry.map_err(|err| err.to_string())?;
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
            if !matches!(ext, "html" | "js" | "css" | "wasm" | "svg" | "json" | "txt") {
                continue;
            }
            let len = entry.metadata().map_err(|err| err.to_string())?.len();
            gzip_file(&path)?;
            if len <= 32 * 1024 * 1024 {
                brotli_file(&path)?;
            }
        }
    }
    Ok(())
}

fn gzip_file(path: &Path) -> Result<(), String> {
    let input = File::open(path).map_err(|err| err.to_string())?;
    let output = File::create(append_suffix(path, ".gz")).map_err(|err| err.to_string())?;
    let mut encoder = flate2::write::GzEncoder::new(output, flate2::Compression::new(6));
    io::copy(&mut io::BufReader::new(input), &mut encoder).map_err(|err| err.to_string())?;
    encoder.finish().map_err(|err| err.to_string())?;
    Ok(())
}

fn brotli_file(path: &Path) -> Result<(), String> {
    let mut input = File::open(path).map_err(|err| err.to_string())?;
    let output = File::create(append_suffix(path, ".br")).map_err(|err| err.to_string())?;
    let mut writer = brotli::CompressorWriter::new(output, 4096, 4, 20);
    io::copy(&mut input, &mut writer).map_err(|err| err.to_string())?;
    writer.flush().map_err(|err| err.to_string())?;
    Ok(())
}

fn append_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut os = path.as_os_str().to_os_string();
    os.push(suffix);
    PathBuf::from(os)
}

fn file_contains(file: &mut impl Read, needles: &[Vec<u8>]) -> Result<bool, String> {
    let keep = needles.iter().map(Vec::len).max().unwrap_or(1);
    let mut buf = vec![0u8; 256 * 1024];
    let mut tail = Vec::new();
    loop {
        let n = file.read(&mut buf).map_err(|err| err.to_string())?;
        if n == 0 {
            return Ok(false);
        }
        tail.extend_from_slice(&buf[..n]);
        for needle in needles {
            if tail.windows(needle.len()).any(|window| window == needle.as_slice()) {
                return Ok(true);
            }
        }
        if tail.len() > keep {
            tail.drain(0..tail.len() - keep);
        }
    }
}

pub fn file_sha256(path: &Path) -> Result<String, String> {
    let mut file = File::open(path).map_err(|err| err.to_string())?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = file.read(&mut buf).map_err(|err| err.to_string())?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hex::encode(hasher.finalize()))
}

/// Check an OpenPGP detached signature over `message` with the release's key, and that the key
/// that signed it is one the registry pins (by primary or signing key fingerprint): a release can
/// carry a key, but not choose which key is trusted.
pub fn verify_openpgp(key_armor: &Path, message: &Path, signature: &Path, fingerprints: &[String]) -> Result<(), String> {
    let home = message.parent().unwrap_or(Path::new(".")).join(format!("gpg-{}", std::process::id()));
    let _ = fs::remove_dir_all(&home);
    fs::create_dir_all(&home).map_err(|err| err.to_string())?;
    let result = (|| {
        let gpg = || {
            let mut command = Command::new("gpg");
            command.arg("--batch").arg("--no-default-keyring").arg("--homedir").arg(&home);
            command
        };
        let imported = gpg().arg("--import").arg(key_armor).output().map_err(|_| "gpg is not installed; refusing a signed app update".to_string())?;
        if !imported.status.success() {
            return Err(format!("gpg import failed: {}", String::from_utf8_lossy(&imported.stderr)));
        }
        let verified = gpg().arg("--status-fd").arg("1").arg("--verify").arg(signature).arg(message).output().map_err(|err| err.to_string())?;
        if !verified.status.success() {
            return Err(format!("signature rejected: {}", String::from_utf8_lossy(&verified.stderr)));
        }
        let status = String::from_utf8_lossy(&verified.stdout);
        let signers: Vec<String> = status
            .lines()
            .filter_map(|line| line.strip_prefix("[GNUPG:] VALIDSIG "))
            .flat_map(|rest| {
                let fields: Vec<&str> = rest.split_whitespace().collect();
                [fields.first().copied(), fields.last().copied()].into_iter().flatten().map(|f| f.to_ascii_uppercase()).collect::<Vec<_>>()
            })
            .collect();
        if signers.is_empty() {
            return Err("gpg reported no valid signature".into());
        }
        if fingerprints.iter().any(|pinned| signers.iter().any(|s| s.eq_ignore_ascii_case(pinned))) {
            Ok(())
        } else {
            Err(format!("signed by {}, which the registry does not pin for this app", signers.join(" / ")))
        }
    })();
    let _ = fs::remove_dir_all(&home);
    result
}

/// The primary key fingerprints in an armored OpenPGP key file (the maintainer tool pins them).
pub fn openpgp_fingerprints(key_armor: &Path) -> Result<Vec<String>, String> {
    let home = key_armor.parent().unwrap_or(Path::new(".")).join(format!("gpg-show-{}", std::process::id()));
    let _ = fs::create_dir_all(&home);
    let out = Command::new("gpg").arg("--batch").arg("--homedir").arg(&home).arg("--with-colons").arg("--show-keys").arg(key_armor).output().map_err(|_| "gpg is not installed".to_string())?;
    let _ = fs::remove_dir_all(&home);
    let text = String::from_utf8_lossy(&out.stdout);
    let mut prints = Vec::new();
    let mut after_pub = false;
    for line in text.lines() {
        let fields: Vec<&str> = line.split(':').collect();
        match fields.first().copied() {
            Some("pub") => after_pub = true,
            Some("fpr") if after_pub => {
                if let Some(fpr) = fields.get(9) {
                    prints.push(fpr.to_ascii_uppercase());
                }
                after_pub = false;
            }
            _ => {}
        }
    }
    if prints.is_empty() {
        Err("no OpenPGP key found".into())
    } else {
        Ok(prints)
    }
}

pub(crate) fn with_lock<T>(data: &Path, body: impl FnOnce() -> Result<T, String>) -> Result<T, String> {
    fs::create_dir_all(data.join("apps")).map_err(|err| err.to_string())?;
    let lock_path = data.join("apps").join("manifest.lock");
    let lock = fs::OpenOptions::new().create(true).write(true).truncate(false).open(&lock_path).map_err(|err| err.to_string())?;
    lock.lock_exclusive().map_err(|err| err.to_string())?;
    let result = body();
    drop(lock);
    result
}

pub(crate) fn unix_now() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

async fn download(client: &reqwest::Client, url: &str, dest: &Path, progress: Option<&Progress>) -> Result<(), String> {
    if !(url.starts_with("https://") || artcraft_cloud_types::registry::is_loopback_http(url)) {
        return Err(format!("refusing a download that is not https: {url}"));
    }
    let mut response = client.get(url).send().await.map_err(|err| format!("download {url}: {err}"))?;
    if !response.status().is_success() {
        return Err(format!("download {url}: {}", response.status()));
    }
    let total = response.content_length().filter(|n| *n > 0);
    let mut file = tokio::fs::File::create(dest).await.map_err(|err| err.to_string())?;
    let mut done: u64 = 0;
    let mut shown = 0u8;
    while let Some(chunk) = response.chunk().await.map_err(|err| err.to_string())? {
        tokio::io::AsyncWriteExt::write_all(&mut file, &chunk).await.map_err(|err| err.to_string())?;
        done += chunk.len() as u64;
        if done > MAX_UNPACKED {
            return Err(format!("{url} is larger than 800 MiB"));
        }
        if let (Some(progress), Some(total)) = (progress, total) {
            let percent = ((done.min(total) * 100) / total) as u8;
            if percent != shown {
                shown = percent;
                progress("downloading", Some(percent));
            }
        }
    }
    file.sync_all().await.map_err(|err| err.to_string())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use artcraft_cloud_types::registry::{ReleaseKeys, VersionStatus, REGISTRY_SCHEMA};
    use zip::write::SimpleFileOptions;
    use zip::ZipWriter;

    const MAILCRAFT_KEYS: [&str; 2] = ["Mu5RF5oJpFhDGfa5wpgznC39tDC8vbUbyNkiAMdluPQ=", "XV6WRgW06/mGhZCHhFVKMIEZDmLevQduZPLSj/UW5zI="];

    #[test]
    fn mailcraft_release_signature() {
        // SHA256SUMS.txt and its signature from MailCraft v0.15.0, checked by hand on 2026-11-24.
        let sums = include_bytes!("../tests/fixtures/mailcraft-0.15.0-SHA256SUMS.txt");
        let sig = include_str!("../tests/fixtures/mailcraft-0.15.0-SHA256SUMS.txt.sig");
        let keys: Vec<String> = MAILCRAFT_KEYS.iter().map(|k| k.to_string()).collect();
        verify_ed25519(&keys, sums, sig).unwrap();
        let mut tampered = sums.to_vec();
        tampered[0] ^= 1;
        assert!(verify_ed25519(&keys, &tampered, sig).is_err());
        assert!(verify_ed25519(&keys[1..], sums, sig).is_err(), "the spare key did not sign it");
        assert!(verify_ed25519(&keys, sums, "not base64!").is_err());
        // The registry pins the same keys for MailCraft.
        let embedded: Registry = serde_json::from_slice(crate::registry::EMBEDDED_JSON).unwrap();
        assert_eq!(embedded.app("mailcraft").unwrap().keys.ed25519, keys);
    }

    #[test]
    fn sums_parser_accepts_binary_marker() {
        let text = "abc  other.zip\nabcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789 *wordcraft-web-1.2.3.zip\n";
        assert!(expected_sha256(text, "missing.zip").is_err());
        assert_eq!(
            expected_sha256("0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef  wordcraft-web-1.2.3.zip\n", "wordcraft-web-1.2.3.zip").unwrap(),
            "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
        );
    }

    #[test]
    fn install_rollback_and_zip_slip() {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path();
        let zip1 = data.join("one.zip");
        write_app_zip(&zip1, "wordcraft", "one");
        let sha1 = file_sha256(&zip1).unwrap();
        install_zip(data, "wordcraft", true, "0.0.1", &zip1, &sha1, "registry").unwrap();
        assert!(version_dir(data, "wordcraft", "0.0.1").join("index.html").is_file());
        assert!(version_dir(data, "wordcraft", "0.0.1").join("index.html.gz").is_file());

        let zip2 = data.join("two.zip");
        write_app_zip(&zip2, "wordcraft", "two");
        let sha2 = file_sha256(&zip2).unwrap();
        install_zip(data, "wordcraft", true, "0.0.2", &zip2, &sha2, "registry").unwrap();
        let manifest = read_manifest(data);
        let row = manifest.apps.get("wordcraft").unwrap();
        assert_eq!(row.version, "0.0.2");
        assert_eq!(row.previous.as_deref(), Some("0.0.1"));
        rollback(data, None, "wordcraft").unwrap();
        assert_eq!(read_manifest(data).apps.get("wordcraft").unwrap().version, "0.0.1");

        let evil = data.join("evil.zip");
        let mut writer = ZipWriter::new(File::create(&evil).unwrap());
        writer.start_file("../evil.txt", SimpleFileOptions::default()).unwrap();
        writer.write_all(b"wordcraft:ready host=parent").unwrap();
        writer.finish().unwrap();
        let sha = file_sha256(&evil).unwrap();
        assert!(install_zip(data, "wordcraft", true, "9.9.9", &evil, &sha, "registry").is_err());

        // Without the bridge marker an app that needs it is refused.
        let plain = data.join("plain.zip");
        let mut writer = ZipWriter::new(File::create(&plain).unwrap());
        writer.start_file("index.html", SimpleFileOptions::default()).unwrap();
        writer.write_all(b"<html></html>").unwrap();
        writer.finish().unwrap();
        let sha = file_sha256(&plain).unwrap();
        assert!(install_zip(data, "wordcraft", true, "9.9.8", &plain, &sha, "registry").is_err());

        remove(data, "wordcraft").unwrap();
        let manifest = read_manifest(data);
        assert!(!manifest.apps.contains_key("wordcraft") && manifest.removed == vec!["wordcraft".to_string()]);
        assert!(!data.join("apps/wordcraft").exists());
    }

    fn registry_with(versions: &[(&str, VersionStatus, bool)]) -> Registry {
        Registry {
            schema: REGISTRY_SCHEMA,
            serial: 1,
            issued: 1,
            expires: i64::MAX,
            apps: vec![RegistryApp {
                id: "wordcraft".into(),
                title: "WordCraft".into(),
                publisher: "t".into(),
                owner: "o".into(),
                repo: "r".into(),
                blurb: String::new(),
                icon: None,
                extensions: vec!["docx".into()],
                author_query: false,
                chrome_query: false,
                bridge: true,
                default_install: true,
                keys: ReleaseKeys::default(),
                recommended: None,
                versions: versions
                    .iter()
                    .map(|(v, status, security)| RegistryVersion {
                        version: v.to_string(),
                        tag: format!("v{v}"),
                        asset: format!("wordcraft-web-{v}.zip"),
                        url: None,
                        sha256: "a".repeat(64),
                        size: 0,
                        signed: "none".into(),
                        status: *status,
                        reason: (*status != VersionStatus::Ok).then(|| "test block".to_string()),
                        security: *security,
                        min_cloud: None,
                        max_cloud: None,
                    })
                    .collect(),
            }],
        }
    }

    #[test]
    fn blocked_versions_roll_back_or_disable() {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path();
        for (v, marker) in [("0.0.1", "one"), ("0.0.2", "two")] {
            let zip = data.join(format!("{v}.zip"));
            write_app_zip(&zip, "wordcraft", marker);
            let sha = file_sha256(&zip).unwrap();
            install_zip(data, "wordcraft", true, v, &zip, &sha, "registry").unwrap();
        }
        // A block that is not about security only warns (the default action).
        let reg = registry_with(&[("0.0.1", VersionStatus::Ok, false), ("0.0.2", VersionStatus::Blocked, false)]);
        assert!(enforce(data, &reg, BlockAction::Security).is_empty());
        assert_eq!(read_manifest(data).apps["wordcraft"].version, "0.0.2");
        // With `warn`, not even a security block is acted on.
        let reg = registry_with(&[("0.0.1", VersionStatus::Ok, false), ("0.0.2", VersionStatus::Blocked, true)]);
        assert!(enforce(data, &reg, BlockAction::Warn).is_empty());
        // A security block rolls back to the allowed previous version.
        let done = enforce(data, &reg, BlockAction::Security);
        assert_eq!(done.len(), 1, "{done:?}");
        let row = read_manifest(data).apps["wordcraft"].clone();
        assert_eq!((row.version.as_str(), row.previous.as_deref()), ("0.0.1", Some("0.0.2")));
        assert!(row.notice.unwrap().contains("Rolled back automatically") && row.disabled.is_none());
        // Rolling forward onto the blocked version is refused.
        assert!(rollback(data, Some(&reg), "wordcraft").unwrap_err().contains("blocked"));
        // When the previous version is blocked too, the app is disabled instead.
        let reg = registry_with(&[("0.0.1", VersionStatus::Blocked, true), ("0.0.2", VersionStatus::Blocked, true)]);
        enforce(data, &reg, BlockAction::Security);
        let row = read_manifest(data).apps["wordcraft"].clone();
        assert_eq!(row.version, "0.0.1");
        assert!(row.disabled.unwrap().contains("blocked"));
        // An incompatible or blocked version is not installed.
        let app = &reg.apps[0];
        assert!(choose(app, Some("0.0.2")).unwrap_err().contains("blocked"));
        assert!(choose(app, None).is_err(), "nothing allowed");
        let reg = registry_with(&[("0.0.1", VersionStatus::Incompatible, false), ("0.0.3", VersionStatus::Ok, false)]);
        assert!(choose(&reg.apps[0], Some("0.0.1")).unwrap_err().contains("compatible"));
        assert_eq!(choose(&reg.apps[0], None).unwrap().version, "0.0.3");
        assert_eq!(update_for(&reg.apps[0], "0.0.1").as_deref(), Some("0.0.3"));
        assert_eq!(update_for(&reg.apps[0], "0.0.3"), None);
    }

    #[test]
    fn openpgp_signature_needs_a_pinned_key() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("home");
        fs::create_dir_all(&home).unwrap();
        let batch = dir.path().join("batch");
        fs::write(&batch, "Key-Type: eddsa\nKey-Curve: ed25519\nName-Real: artcraft test\nName-Email: test@example.com\nExpire-Date: 0\n%no-protection\n%commit\n").unwrap();
        let status = Command::new("gpg").arg("--batch").arg("--homedir").arg(&home).arg("--gen-key").arg(&batch).status().expect("gpg");
        assert!(status.success(), "gpg gen-key");
        let key = dir.path().join("key.asc");
        let key_bytes = Command::new("gpg").arg("--batch").arg("--homedir").arg(&home).arg("--export").arg("--armor").output().unwrap();
        assert!(key_bytes.status.success());
        fs::write(&key, key_bytes.stdout).unwrap();
        let prints = openpgp_fingerprints(&key).unwrap();
        assert_eq!(prints.len(), 1);
        assert_eq!(prints[0].len(), 40);
        let message = dir.path().join("msg");
        fs::write(&message, b"checksums").unwrap();
        let sig = dir.path().join("msg.asc");
        let signed = Command::new("gpg").arg("--batch").arg("--homedir").arg(&home).arg("--armor").arg("--detach-sign").arg("--output").arg(&sig).arg(&message).status().unwrap();
        assert!(signed.success());
        verify_openpgp(&key, &message, &sig, &prints).unwrap();
        assert!(verify_openpgp(&key, &message, &sig, &["0".repeat(40)]).unwrap_err().contains("does not pin"), "a key the release carries is not trusted by itself");
        fs::write(&message, b"tampered").unwrap();
        assert!(verify_openpgp(&key, &message, &sig, &prints).is_err());
    }

    pub(crate) fn write_app_zip(path: &Path, app: &str, marker: &str) {
        let mut writer = ZipWriter::new(File::create(path).unwrap());
        writer.start_file(format!("{app}-web/index.html"), SimpleFileOptions::default()).unwrap();
        writer.write_all(format!("<html>{app}:ready host=parent {marker}</html>").as_bytes()).unwrap();
        writer.start_file(format!("{app}-web/app.js"), SimpleFileOptions::default()).unwrap();
        writer.write_all(b"console.log('ok')").unwrap();
        writer.finish().unwrap();
    }
}
