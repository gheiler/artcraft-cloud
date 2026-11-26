//! `artcraft-cloud apps …`: the same installs, updates and registry checks as the Apps page.

use std::path::Path;

use artcraft_cloud_server::apps::{self, no_progress};
use artcraft_cloud_server::registry::{cloud_version, Loaded, RegistryStore};
use artcraft_cloud_server::{read_manifest, Config};
use artcraft_cloud_types::registry::{compare_versions, Verdict};

fn store(config: &Config) -> RegistryStore {
    RegistryStore::new(&config.data_dir, config.registry.clone())
}

fn current(store: &RegistryStore) -> Result<std::sync::Arc<Loaded>, String> {
    store.current().ok_or_else(|| "no app registry is available: run `artcraft-cloud apps registry refresh`".to_string())
}

/// Refresh quietly first, so a command works from the newest registry when the network is there.
async fn fresh(config: &Config) -> Result<std::sync::Arc<Loaded>, String> {
    let store = store(config);
    if let Err(err) = store.refresh().await {
        eprintln!("registry refresh failed ({err}); using the registry at hand");
    }
    if let Some(loaded) = store.current() {
        for line in apps::enforce(&config.data_dir, &loaded.registry, config.blocked_apps) {
            println!("{line}");
        }
    }
    current(&store)
}

pub async fn update(config: &Config, only: Option<&str>, version: Option<&str>) -> Result<(), String> {
    let loaded = fresh(config).await?;
    let data = &config.data_dir;
    if let Some(id) = only {
        if read_manifest(data).apps.get(id).is_some_and(|row| row.is_sideload()) && loaded.registry.app(id).is_none() {
            let (version, sha) = apps::update_from_source(data, id, version, Some(&loaded.registry), no_progress()).await?;
            println!("{id} {version} installed from its source (sha256 {sha})");
            return Ok(());
        }
        let (version, signature) = apps::install(data, &loaded, id, version, no_progress()).await?;
        println!("{id} {version} installed ({signature})");
        return Ok(());
    }
    let manifest = read_manifest(data);
    let mut errors = Vec::new();
    for app in &loaded.registry.apps {
        let installed = manifest.apps.get(&app.id);
        let wanted = match installed {
            Some(row) if row.disabled.is_some() => continue,
            Some(row) => apps::update_for(app, &row.version),
            None if app.default_install && !manifest.removed.contains(&app.id) => app.target(cloud_version()).map(|v| v.version.clone()),
            None => None,
        };
        let Some(wanted) = wanted else {
            if let Some(row) = installed {
                println!("{} {} is up to date", app.id, row.version);
            }
            continue;
        };
        match apps::install(data, &loaded, &app.id, Some(&wanted), no_progress()).await {
            Ok((version, signature)) => println!("{} {version} installed ({signature})", app.id),
            Err(err) => errors.push(format!("{}: {err}", app.id)),
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.join("; "))
    }
}

pub fn rollback(config: &Config, app: &str) -> Result<(), String> {
    let loaded = store(config).current();
    let version = apps::rollback(&config.data_dir, loaded.as_ref().map(|l| &l.registry), app)?;
    println!("{app} is back on {version}");
    Ok(())
}

pub fn install_local(config: &Config, app: &str, version: &str, zip: &Path) -> Result<(), String> {
    let loaded = current(&store(config))?;
    let entry = loaded.registry.app(app).ok_or_else(|| format!("{app} is not in the app registry"))?;
    apps::install_local(&config.data_dir, app, entry.bridge, version, zip)
}

pub fn remove(config: &Config, app: &str) -> Result<(), String> {
    apps::remove(&config.data_dir, app)?;
    println!("{app} removed (documents stay; `apps update {app}` adds it again)");
    Ok(())
}

pub fn settings(config: &Config, app: &str, auto_update: Option<bool>, disabled: Option<bool>) -> Result<(), String> {
    apps::set_settings(&config.data_dir, app, auto_update, disabled, false)?;
    println!("{app}: settings saved");
    Ok(())
}

pub fn list(config: &Config) -> Result<(), String> {
    let loaded = store(config).current();
    let manifest = read_manifest(&config.data_dir);
    let cloud = cloud_version();
    println!("{:<14} {:<10} {:<13} {:<10} {:<10} notes", "app", "installed", "state", "update", "previous");
    let mut ids: Vec<String> = loaded.as_ref().map(|l| l.registry.apps.iter().map(|a| a.id.clone()).collect()).unwrap_or_default();
    ids.extend(manifest.apps.keys().filter(|id| !ids.contains(id)).cloned().collect::<Vec<_>>());
    for id in ids {
        let app = loaded.as_ref().and_then(|l| l.registry.app(&id));
        let row = manifest.apps.get(&id);
        let verdict = match (app, row) {
            (Some(a), Some(r)) => a.verdict(&r.version, cloud),
            (None, Some(_)) => Verdict::Unlisted,
            _ => {
                let offer = app.and_then(|a| a.target(cloud)).map(|v| v.version.clone()).unwrap_or_default();
                println!("{id:<14} {:<10} {:<13} {offer:<10} {:<10}", "-", "available", "-");
                continue;
            }
        };
        let row = row.expect("installed");
        let update = app.and_then(|a| apps::update_for(a, &row.version)).unwrap_or_else(|| "-".into());
        let mut notes = Vec::new();
        if let Some(reason) = verdict.reason() {
            notes.push(reason);
        }
        if let Some(why) = &row.disabled {
            notes.push(format!("disabled: {why}"));
        }
        if row.auto_update {
            notes.push("auto-update".into());
        }
        if let Some(notice) = &row.notice {
            notes.push(notice.clone());
        }
        println!("{id:<14} {:<10} {:<13} {update:<10} {:<10} {}", row.version, verdict.label(), row.previous.as_deref().unwrap_or("-"), notes.join("; "));
    }
    Ok(())
}

pub async fn registry_refresh(config: &Config) -> Result<(), String> {
    let store = store(config);
    let result = store.refresh().await;
    if let Some(loaded) = store.current() {
        for line in apps::enforce(&config.data_dir, &loaded.registry, config.blocked_apps) {
            println!("{line}");
        }
    }
    let loaded = result?;
    println!("registry serial {} from {} accepted (signature ok, expires {})", loaded.registry.serial, store.config.url, loaded.registry.expires);
    Ok(())
}

pub fn registry_show(config: &Config) -> Result<(), String> {
    let store = store(config);
    let status = store.status();
    println!("url          {}", status.url);
    println!("keys         {}", if status.custom_keys { "ARTCRAFT_REGISTRY_KEYS (self-hosted)" } else { "built in" });
    println!("in use       {} (serial {}, signature {})", status.source, status.serial, if status.signature_ok { "ok" } else { "none usable" });
    println!("issued       {}", status.issued);
    println!("expires      {}{}", status.expires, if status.expired { " (EXPIRED)" } else { "" });
    println!("last fetch   {}", status.last_fetch.map(|t| t.to_string()).unwrap_or_else(|| "never".into()));
    if let Some(err) = &status.last_error {
        println!("last error   {err}");
    }
    if let Some(loaded) = store.current() {
        let cloud = cloud_version();
        for app in &loaded.registry.apps {
            let mut versions: Vec<_> = app.versions.iter().collect();
            versions.sort_by(|a, b| compare_versions(&b.version, &a.version));
            let list: Vec<String> = versions.iter().map(|v| format!("{} {}", v.version, v.verdict(cloud).label())).collect();
            println!("  {:<14} {}", app.id, list.join(", "));
        }
    }
    Ok(())
}

pub struct SideloadArgs {
    pub zip: Option<std::path::PathBuf>,
    pub id: String,
    pub name: String,
    pub version: String,
    pub publisher: String,
    pub blurb: String,
    pub ext: Vec<String>,
    pub bridge: bool,
    pub claim_defaults: bool,
    pub confirm: bool,
    pub confirm_conflicts: bool,
    pub github: Option<String>,
    pub manifest: Option<String>,
    pub isolation: String,
    pub confirm_regular: bool,
}

pub async fn sideload(config: &Config, args: SideloadArgs) -> Result<(), String> {
    if !args.confirm {
        return Err("pass --confirm to acknowledge that this build is not on the artcraftCloud trust list".into());
    }
    let id = artcraft_cloud_server::sideload::normalize_id(&args.id)?;
    let version = artcraft_cloud_server::sideload::normalize_version(&args.version)?;
    let extensions = artcraft_cloud_server::sideload::normalize_extensions(&args.ext)?;
    let grants = artcraft_cloud_server::sideload::normalize_grants(&[], args.bridge)?;
    let isolation = artcraft_cloud_server::sideload::normalize_isolation(
        Some(args.isolation.as_str()).filter(|s| !s.is_empty()),
        config.apps_origin().is_some(),
        args.confirm_regular,
    )?;
    let source = match (args.github.as_ref(), args.manifest.as_ref()) {
        (Some(repo), None) => artcraft_cloud_server::sideload::normalize_source(Some(&artcraft_cloud_types::AppSource { kind: "github".into(), url: repo.clone() }))?,
        (None, Some(url)) => artcraft_cloud_server::sideload::normalize_source(Some(&artcraft_cloud_types::AppSource { kind: "manifest".into(), url: url.clone() }))?,
        (Some(_), Some(_)) => return Err("pass --github or --manifest, not both".into()),
        _ => None,
    };
    let loaded = store(config).current();
    let registry = loaded.as_ref().map(|l| &l.registry);
    let manifest = read_manifest(&config.data_dir);
    if (manifest.apps.get(&id).is_some_and(|row| !row.is_sideload()) || registry.is_some_and(|r| r.app(&id).is_some())) && !args.confirm_conflicts {
        return Err(format!("{id} is on the official app registry; pass --confirm-conflicts to replace it with a sideloaded build"));
    }
    let conflicts = artcraft_cloud_server::sideload::type_conflicts(&manifest, registry, &id, &extensions, args.claim_defaults);
    if !conflicts.is_empty() && !args.confirm_conflicts {
        let list: Vec<String> = conflicts.iter().map(|c| format!("{} ({})", c.ext, c.title)).collect();
        return Err(format!("file type already in use: {}; pass --confirm-conflicts to continue", list.join(", ")));
    }
    let meta = artcraft_cloud_server::sideload::meta(&args.name, &args.publisher, &args.blurb, args.bridge, extensions, grants, args.claim_defaults, source, isolation.clone())?;
    let zip = match args.zip {
        Some(path) => path,
        None => return Err("pass a zip path, or use the Apps page to install the first version from a source".into()),
    };
    if !zip.is_file() {
        return Err(format!("{} is not a file", zip.display()));
    }
    let sha = apps::install_sideload(&config.data_dir, &id, &version, &zip, meta, registry)?;
    let _ = log_audit(config, "install", &id, Some(&version), Some(&sha), Some(&format!("sideload,isolation={isolation}")));
    println!("{id} {version} sideloaded (sha256 {sha}, unverified, isolation {isolation})");
    Ok(())
}

pub fn set_source(config: &Config, app: &str, github: Option<String>, manifest: Option<String>, clear: bool) -> Result<(), String> {
    let source = if clear {
        None
    } else if let Some(repo) = github {
        artcraft_cloud_server::sideload::normalize_source(Some(&artcraft_cloud_types::AppSource { kind: "github".into(), url: repo }))?
    } else if let Some(url) = manifest {
        artcraft_cloud_server::sideload::normalize_source(Some(&artcraft_cloud_types::AppSource { kind: "manifest".into(), url }))?
    } else {
        return Err("pass --github owner/repo, --manifest URL, or --clear".into());
    };
    apps::set_sideload_source(&config.data_dir, app, source)?;
    let _ = log_audit(config, "source", app, None, None, Some("cli"));
    println!("{app}: source saved");
    Ok(())
}

pub async fn check_source(config: &Config, app: &str) -> Result<(), String> {
    let row = read_manifest(&config.data_dir).apps.get(app).cloned().ok_or_else(|| format!("{app} is not installed"))?;
    let source = row.sideload.as_ref().and_then(|m| m.source.clone()).ok_or_else(|| format!("{app} has no source to check"))?;
    let found = artcraft_cloud_server::sideload::check_source(app, &source).await?;
    if let Some(newest) = artcraft_cloud_server::sideload::newest(&found) {
        apps::record_offered(&config.data_dir, app, artcraft_cloud_server::sideload::offered_from(newest))?;
        println!("{app}: latest on the source is {} (sha256 {})", newest.version, newest.sha256);
        if artcraft_cloud_types::registry::compare_versions(&newest.version, &row.version) == std::cmp::Ordering::Greater {
            println!("a newer version is available; `apps update {app} --version {}` (or the Apps page) installs it", newest.version);
        }
    } else {
        println!("{app}: the source listed no versions");
    }
    Ok(())
}

pub fn set_types(config: &Config, app: &str, ext: Vec<String>, claim_defaults: bool, confirm_conflicts: bool) -> Result<(), String> {
    let extensions = artcraft_cloud_server::sideload::normalize_extensions(&ext)?;
    let loaded = store(config).current();
    let registry = loaded.as_ref().map(|l| &l.registry);
    let conflicts = artcraft_cloud_server::sideload::type_conflicts(&read_manifest(&config.data_dir), registry, app, &extensions, claim_defaults);
    if !conflicts.is_empty() && !confirm_conflicts {
        let list: Vec<String> = conflicts.iter().map(|c| format!("{} ({})", c.ext, c.title)).collect();
        return Err(format!("file type already in use: {}; pass --confirm-conflicts to continue", list.join(", ")));
    }
    apps::set_sideload_meta(&config.data_dir, app, Some(extensions), None, Some(claim_defaults), None, registry)?;
    let _ = log_audit(config, "settings", app, None, None, Some("types"));
    println!("{app}: file types saved");
    Ok(())
}

pub fn set_grants(config: &Config, app: &str, grants: Vec<String>) -> Result<(), String> {
    let row = read_manifest(&config.data_dir).apps.get(app).cloned().ok_or_else(|| format!("{app} is not installed"))?;
    let bridge = row.sideload.as_ref().map(|m| m.bridge).unwrap_or(false);
    let grants = artcraft_cloud_server::sideload::normalize_grants(&grants, bridge)?;
    let loaded = store(config).current();
    apps::set_sideload_meta(&config.data_dir, app, None, Some(grants), None, None, loaded.as_ref().map(|l| &l.registry))?;
    let _ = log_audit(config, "settings", app, None, None, Some("grants"));
    println!("{app}: grants saved");
    Ok(())
}

pub fn set_isolation(config: &Config, app: &str, isolation: &str, confirm_regular: bool) -> Result<(), String> {
    let row = read_manifest(&config.data_dir).apps.get(app).cloned().ok_or_else(|| format!("{app} is not installed"))?;
    if !row.is_sideload() {
        return Err(format!("{app} is not a sideloaded app"));
    }
    let current = row.sideload.as_ref().map(artcraft_cloud_server::sideload::isolation_of).unwrap_or_else(|| "sandbox".into());
    let confirm = confirm_regular || current == "regular";
    let mode = artcraft_cloud_server::sideload::normalize_isolation(Some(isolation), config.apps_origin().is_some(), confirm)?;
    apps::set_sideload_meta(&config.data_dir, app, None, None, None, Some(mode.clone()), None)?;
    let _ = log_audit(config, "isolation", app, None, None, Some(&format!("isolation={mode}")));
    println!("{app}: isolation set to {mode}");
    Ok(())
}

pub fn audit(config: &Config) -> Result<(), String> {
    let db = artcraft_cloud_server::Db::open(&config.data_dir.join("db.sqlite")).map_err(|err| err.to_string())?;
    let events = db.list_app_audit(80).map_err(|err| err.to_string())?;
    if events.is_empty() {
        println!("no app audit events yet");
        return Ok(());
    }
    println!("{:<6} {:<12} {:<10} {:<16} {:<10} detail", "id", "at", "action", "app", "version");
    for e in events {
        println!("{:<6} {:<12} {:<10} {:<16} {:<10} {}", e.id, e.at, e.action, e.app, e.version.as_deref().unwrap_or("-"), e.detail.as_deref().or(e.source.as_deref()).unwrap_or("-"));
    }
    Ok(())
}

fn log_audit(config: &Config, action: &str, app: &str, version: Option<&str>, sha256: Option<&str>, detail: Option<&str>) -> Result<(), String> {
    let db = artcraft_cloud_server::Db::open(&config.data_dir.join("db.sqlite")).map_err(|err| err.to_string())?;
    db.append_app_audit("cli", action, app, version, sha256, None, detail).map_err(|err| err.to_string())
}
