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
