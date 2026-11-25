use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Manifest {
    pub apps: BTreeMap<String, InstalledApp>,
    /// Apps an admin removed: the start-up check does not install them again.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub removed: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstalledApp {
    pub version: String,
    pub previous: Option<String>,
    pub sha256: String,
    pub signature: String,
    pub installed_at: i64,
    /// Why the app is disabled (hidden from everyone and not served).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disabled: Option<String>,
    /// Updated by itself to the version the registry points to.
    #[serde(default)]
    pub auto_update: bool,
    /// Something done automatically (a rollback for a blocked version) for the admin to read.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notice: Option<String>,
}

pub fn apps_root(data: &Path) -> PathBuf {
    data.join("apps")
}

pub fn manifest_path(data: &Path) -> PathBuf {
    apps_root(data).join("manifest.json")
}

pub fn version_dir(data: &Path, id: &str, version: &str) -> PathBuf {
    apps_root(data).join(id).join(version)
}

pub fn read_manifest(data: &Path) -> Manifest {
    let path = manifest_path(data);
    let Ok(text) = std::fs::read_to_string(&path) else {
        return Manifest::default();
    };
    serde_json::from_str(&text).unwrap_or_default()
}

pub fn write_manifest(data: &Path, manifest: &Manifest) -> Result<(), String> {
    let path = manifest_path(data);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|err| err.to_string())?;
    }
    let text = serde_json::to_string_pretty(manifest).map_err(|err| err.to_string())?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, text).map_err(|err| err.to_string())?;
    std::fs::rename(&tmp, &path).map_err(|err| err.to_string())?;
    Ok(())
}

pub fn current_dir(data: &Path, id: &str) -> Option<PathBuf> {
    let manifest = read_manifest(data);
    let installed = manifest.apps.get(id)?;
    if installed.disabled.is_some() {
        return None;
    }
    let dir = version_dir(data, id, &installed.version);
    dir.is_dir().then_some(dir)
}
