use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Manifest {
    pub apps: BTreeMap<String, InstalledApp>,
    /// Apps an admin removed: the start-up check does not install them again.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub removed: Vec<String>,
    /// Default handler for a file suffix (without a dot) → app id. Official apps win unless a
    /// sideload was explicitly allowed to claim the type.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub defaults: BTreeMap<String, String>,
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
    /// Updated by itself to the version the registry (or a connected source) points to.
    #[serde(default)]
    pub auto_update: bool,
    /// Something done automatically (a rollback for a blocked version) for the admin to read.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notice: Option<String>,
    /// Present when this install came from the Danger zone (a zip or a connected source).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sideload: Option<SideloadMeta>,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub size: u64,
}

fn is_zero(value: &u64) -> bool {
    *value == 0
}

fn default_sandbox() -> String {
    "sandbox".into()
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SideloadMeta {
    pub name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub publisher: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub blurb: String,
    pub bridge: bool,
    #[serde(default)]
    pub extensions: Vec<String>,
    /// Capability grants. `document` is implied for a bridge app.
    #[serde(default)]
    pub grants: Vec<String>,
    /// Become the default handler even when an official app already claims a type.
    #[serde(default)]
    pub claim_defaults: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<SideloadSource>,
    /// Last version a source check found (newer than the installed one, or the same).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offered: Option<OfferedVersion>,
    /// `sandbox` (default), `regular` or `domain`.
    #[serde(default = "default_sandbox")]
    pub isolation: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct OfferedVersion {
    pub version: String,
    pub url: String,
    pub sha256: String,
    #[serde(default)]
    pub signed: String,
    pub checked_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct SideloadSource {
    /// `github` or `manifest`.
    pub kind: String,
    /// GitHub `owner/repo` or the https (or loopback http) manifest URL.
    pub url: String,
}

impl InstalledApp {
    pub fn is_sideload(&self) -> bool {
        self.sideload.is_some()
    }
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
