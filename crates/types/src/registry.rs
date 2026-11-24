//! The app registry: the public, signed list of the Crafting Apps artcraftCloud may install, the
//! versions it may install of each, and what is known to be wrong with the others.
//!
//! The file is `registry/apps.json` in the artcraft-cloud repository, signed with the registry's
//! ed25519 key as `apps.json.sig` (base64 of the signature over the exact bytes). The server checks
//! the signature (it has no crypto here: this crate is shared with the wasm shell); this module
//! holds the format and the rules that decide which version is allowed.

use std::cmp::Ordering;

use serde::{Deserialize, Serialize};

/// The format this build reads. A registry with a higher schema is refused rather than misread.
pub const REGISTRY_SCHEMA: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Registry {
    pub schema: u32,
    /// Grows with every change; a registry with a lower serial than one already seen is refused.
    pub serial: u64,
    /// Unix seconds.
    pub issued: i64,
    /// Unix seconds. An expired registry is not accepted from the network, and nothing new is
    /// installed while the only one at hand has expired.
    pub expires: i64,
    pub apps: Vec<RegistryApp>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RegistryApp {
    pub id: String,
    pub title: String,
    /// Who makes it, as shown to the admin.
    pub publisher: String,
    /// GitHub owner and repository of the releases.
    pub owner: String,
    pub repo: String,
    pub blurb: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<RegistryIcon>,
    #[serde(default)]
    pub extensions: Vec<String>,
    /// The app reads `?author=` for comments and tracked changes.
    #[serde(default)]
    pub author_query: bool,
    /// The app understands `?chrome=nofiletab`.
    #[serde(default)]
    pub chrome_query: bool,
    /// The web build speaks the `craft-web-host` contract (opens and saves drive files).
    pub bridge: bool,
    /// Installed by itself on a new install (until an admin removes it).
    #[serde(default)]
    pub default_install: bool,
    #[serde(default)]
    pub keys: ReleaseKeys,
    /// The version admins are pointed to; else the newest allowed one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recommended: Option<String>,
    pub versions: Vec<RegistryVersion>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RegistryIcon {
    pub url: String,
    pub sha256: String,
}

/// The keys the app's own releases are signed with.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReleaseKeys {
    /// ed25519 public keys (base64) that sign `SHA256SUMS.txt` as `SHA256SUMS.txt.sig`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ed25519: Vec<String>,
    /// OpenPGP primary key fingerprints (40 hex) that sign `SHA256SUMS.txt` as `SHA256SUMS.txt.asc`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub openpgp: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RegistryVersion {
    pub version: String,
    /// The release tag, e.g. `v0.18.1`.
    pub tag: String,
    /// The web build archive in the release, e.g. `wordcraft-web-0.18.1.zip`.
    pub asset: String,
    /// Where to download it; default the GitHub release asset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// SHA-256 of the archive (hex). Checked on every install, signed or not.
    pub sha256: String,
    #[serde(default)]
    pub size: u64,
    /// How the release itself is signed: `ed25519`, `openpgp` or `none`.
    pub signed: String,
    #[serde(default)]
    pub status: VersionStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// A block for a security reason: installs running it are rolled back or the app is disabled.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub security: bool,
    /// The artcraftCloud versions it works with (inclusive).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_cloud: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_cloud: Option<String>,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum VersionStatus {
    #[default]
    Ok,
    Blocked,
    Incompatible,
}

/// What the registry says about one version for this artcraftCloud.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    Allowed,
    Blocked { reason: String, security: bool },
    Incompatible { reason: String },
    /// Not in the registry (installed before, or by hand).
    Unlisted,
}

impl Verdict {
    pub fn allowed(&self) -> bool {
        matches!(self, Verdict::Allowed)
    }

    pub fn label(&self) -> &'static str {
        match self {
            Verdict::Allowed => "ok",
            Verdict::Blocked { .. } => "blocked",
            Verdict::Incompatible { .. } => "incompatible",
            Verdict::Unlisted => "unlisted",
        }
    }

    pub fn reason(&self) -> Option<String> {
        match self {
            Verdict::Blocked { reason, .. } | Verdict::Incompatible { reason } => Some(reason.clone()),
            _ => None,
        }
    }
}

impl Registry {
    pub fn app(&self, id: &str) -> Option<&RegistryApp> {
        self.apps.iter().find(|app| app.id == id)
    }

    pub fn expired(&self, now: i64) -> bool {
        self.expires <= now
    }

    /// Structure checks beyond what serde does, so a mistake in the file is caught where it is
    /// made (`registry verify`, CI) and by every server before it trusts the file.
    pub fn validate(&self) -> Result<(), String> {
        if self.schema != REGISTRY_SCHEMA {
            return Err(format!("registry schema {} is not {REGISTRY_SCHEMA}", self.schema));
        }
        if self.expires <= self.issued {
            return Err("registry expires before it is issued".into());
        }
        let mut ids = std::collections::BTreeSet::new();
        for app in &self.apps {
            if !valid_id(&app.id) {
                return Err(format!("bad app id {:?}", app.id));
            }
            if !ids.insert(app.id.as_str()) {
                return Err(format!("{} is listed twice", app.id));
            }
            if app.title.trim().is_empty() || !valid_slug(&app.owner) || !valid_slug(&app.repo) {
                return Err(format!("{}: title, owner and repo are required", app.id));
            }
            for key in &app.keys.openpgp {
                if key.len() != 40 || !key.chars().all(|c| c.is_ascii_hexdigit()) {
                    return Err(format!("{}: OpenPGP keys are 40-hex fingerprints", app.id));
                }
            }
            if let Some(icon) = &app.icon {
                if !valid_sha256(&icon.sha256) || !(icon.url.starts_with("https://") || is_loopback_http(&icon.url)) {
                    return Err(format!("{}: the icon needs an https URL and a sha256", app.id));
                }
            }
            let mut versions = std::collections::BTreeSet::new();
            for v in &app.versions {
                let at = format!("{} {}", app.id, v.version);
                if !valid_version(&v.version) || !versions.insert(v.version.as_str()) {
                    return Err(format!("{at}: bad or repeated version"));
                }
                if !valid_sha256(&v.sha256) {
                    return Err(format!("{at}: sha256 must be 64 hex"));
                }
                if v.asset.is_empty() || v.asset.contains(['/', '\\']) || v.tag.is_empty() || v.tag.contains(['/', '\\', '?', '#']) {
                    return Err(format!("{at}: bad tag or asset"));
                }
                match v.signed.as_str() {
                    "ed25519" if app.keys.ed25519.is_empty() => return Err(format!("{at}: signed with ed25519 but the app lists no ed25519 key")),
                    "openpgp" if app.keys.openpgp.is_empty() => return Err(format!("{at}: signed with OpenPGP but the app lists no fingerprint")),
                    "ed25519" | "openpgp" | "none" => {}
                    other => return Err(format!("{at}: unknown signature kind {other:?}")),
                }
                if v.status != VersionStatus::Ok && v.reason.as_deref().is_none_or(|r| r.trim().is_empty()) {
                    return Err(format!("{at}: a blocked or incompatible version needs a reason"));
                }
                for bound in [&v.min_cloud, &v.max_cloud].into_iter().flatten() {
                    if !valid_version(bound) {
                        return Err(format!("{at}: bad artcraftCloud version bound {bound:?}"));
                    }
                }
                if let Some(url) = &v.url {
                    if !(url.starts_with("https://") || is_loopback_http(url)) {
                        return Err(format!("{at}: the download URL must be https"));
                    }
                }
            }
            if let Some(rec) = &app.recommended {
                if !versions.contains(rec.as_str()) {
                    return Err(format!("{}: recommended {rec} is not listed", app.id));
                }
            }
        }
        Ok(())
    }
}

impl RegistryApp {
    pub fn version(&self, version: &str) -> Option<&RegistryVersion> {
        self.versions.iter().find(|v| v.version == version)
    }

    pub fn verdict(&self, version: &str, cloud: &str) -> Verdict {
        match self.version(version) {
            Some(v) => v.verdict(cloud),
            None => Verdict::Unlisted,
        }
    }

    /// The newest version this artcraftCloud may install.
    pub fn latest_allowed(&self, cloud: &str) -> Option<&RegistryVersion> {
        self.versions.iter().filter(|v| v.verdict(cloud).allowed()).max_by(|a, b| compare_versions(&a.version, &b.version))
    }

    /// The recommended version when it is allowed here, else the newest allowed one.
    pub fn target(&self, cloud: &str) -> Option<&RegistryVersion> {
        self.recommended.as_deref().and_then(|rec| self.version(rec)).filter(|v| v.verdict(cloud).allowed()).or_else(|| self.latest_allowed(cloud))
    }

    /// The release archive's download address.
    pub fn download_url(&self, v: &RegistryVersion) -> String {
        v.url.clone().unwrap_or_else(|| format!("https://github.com/{}/{}/releases/download/{}/{}", self.owner, self.repo, v.tag, v.asset))
    }

    /// The address of another file of the same release (`SHA256SUMS.txt` and its signature).
    pub fn release_file_url(&self, v: &RegistryVersion, name: &str) -> String {
        let url = self.download_url(v);
        match url.rsplit_once('/') {
            Some((base, _)) => format!("{base}/{name}"),
            None => name.to_string(),
        }
    }
}

impl RegistryVersion {
    pub fn verdict(&self, cloud: &str) -> Verdict {
        match self.status {
            VersionStatus::Blocked => {
                return Verdict::Blocked { reason: self.reason.clone().unwrap_or_default(), security: self.security };
            }
            VersionStatus::Incompatible => return Verdict::Incompatible { reason: self.reason.clone().unwrap_or_default() },
            VersionStatus::Ok => {}
        }
        if let Some(min) = &self.min_cloud {
            if compare_versions(cloud, min) == Ordering::Less {
                return Verdict::Incompatible { reason: format!("needs artcraftCloud {min} or newer") };
            }
        }
        if let Some(max) = &self.max_cloud {
            if compare_versions(cloud, max) == Ordering::Greater {
                return Verdict::Incompatible { reason: format!("works up to artcraftCloud {max}") };
            }
        }
        Verdict::Allowed
    }
}

/// Dotted versions compared part by part, numbers as numbers (`0.10.0` > `0.9.2`); a pre-release
/// (`1.0.0-rc.1`) sorts before its release.
pub fn compare_versions(a: &str, b: &str) -> Ordering {
    let split = |v: &str| {
        let v = v.trim().trim_start_matches('v');
        let (core, pre) = match v.split_once('-') {
            Some((core, pre)) => (core.to_string(), Some(pre.to_string())),
            None => (v.split('+').next().unwrap_or(v).to_string(), None),
        };
        (core, pre)
    };
    let part_cmp = |x: &str, y: &str| match (x.parse::<u64>(), y.parse::<u64>()) {
        (Ok(x), Ok(y)) => x.cmp(&y),
        (Ok(_), Err(_)) => Ordering::Less,
        (Err(_), Ok(_)) => Ordering::Greater,
        _ => x.cmp(y),
    };
    let (core_a, pre_a) = split(a);
    let (core_b, pre_b) = split(b);
    let mut pa = core_a.split('.');
    let mut pb = core_b.split('.');
    loop {
        match (pa.next(), pb.next()) {
            (None, None) => break,
            (x, y) => {
                let ord = part_cmp(x.unwrap_or("0"), y.unwrap_or("0"));
                if ord != Ordering::Equal {
                    return ord;
                }
            }
        }
    }
    match (pre_a, pre_b) {
        (None, None) => Ordering::Equal,
        (None, Some(_)) => Ordering::Greater,
        (Some(_), None) => Ordering::Less,
        (Some(x), Some(y)) => {
            let mut xa = x.split('.');
            let mut ya = y.split('.');
            loop {
                match (xa.next(), ya.next()) {
                    (None, None) => return Ordering::Equal,
                    (None, Some(_)) => return Ordering::Less,
                    (Some(_), None) => return Ordering::Greater,
                    (Some(p), Some(q)) => {
                        let ord = part_cmp(p, q);
                        if ord != Ordering::Equal {
                            return ord;
                        }
                    }
                }
            }
        }
    }
}

pub fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 40 && id.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-') && !id.starts_with('-')
}

pub fn valid_version(v: &str) -> bool {
    !v.is_empty() && v.len() <= 40 && v.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '+')) && !v.starts_with('.') && !v.contains("..")
}

fn valid_slug(s: &str) -> bool {
    !s.is_empty() && s.len() <= 100 && s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.')) && !s.starts_with('.')
}

pub fn valid_sha256(s: &str) -> bool {
    s.len() == 64 && s.chars().all(|c| c.is_ascii_hexdigit())
}

/// `http://` is accepted only to this computer (a test mirror).
pub fn is_loopback_http(url: &str) -> bool {
    ["http://127.0.0.1:", "http://127.0.0.1/", "http://localhost:", "http://localhost/", "http://[::1]:"].iter().any(|p| url.starts_with(p))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn version(v: &str) -> RegistryVersion {
        RegistryVersion {
            version: v.into(),
            tag: format!("v{v}"),
            asset: format!("demo-web-{v}.zip"),
            url: None,
            sha256: "a".repeat(64),
            size: 0,
            signed: "none".into(),
            status: VersionStatus::Ok,
            reason: None,
            security: false,
            min_cloud: None,
            max_cloud: None,
        }
    }

    fn app(versions: Vec<RegistryVersion>) -> RegistryApp {
        RegistryApp {
            id: "demo".into(),
            title: "Demo".into(),
            publisher: "Demo".into(),
            owner: "o".into(),
            repo: "demo".into(),
            blurb: String::new(),
            icon: None,
            extensions: vec![],
            author_query: false,
            chrome_query: false,
            bridge: true,
            default_install: false,
            keys: ReleaseKeys::default(),
            recommended: None,
            versions,
        }
    }

    #[test]
    fn versions_compare_numerically() {
        assert_eq!(compare_versions("0.10.0", "0.9.2"), Ordering::Greater);
        assert_eq!(compare_versions("v1.2", "1.2.0"), Ordering::Equal);
        assert_eq!(compare_versions("1.0.0-rc.1", "1.0.0"), Ordering::Less);
        assert_eq!(compare_versions("1.0.0-rc.2", "1.0.0-rc.10"), Ordering::Less);
        assert_eq!(compare_versions("0.2.0", "0.15.0"), Ordering::Less);
    }

    #[test]
    fn blocked_and_incompatible_versions_are_not_targets() {
        let mut blocked = version("1.2.0");
        blocked.status = VersionStatus::Blocked;
        blocked.reason = Some("compromised build".into());
        blocked.security = true;
        let mut needs_newer = version("1.3.0");
        needs_newer.min_cloud = Some("0.2.0".into());
        let mut too_old = version("0.9.0");
        too_old.max_cloud = Some("0.0.9".into());
        let mut a = app(vec![version("1.0.0"), version("1.1.0"), blocked, needs_newer, too_old]);
        assert_eq!(a.latest_allowed("0.1.0").unwrap().version, "1.1.0");
        assert!(matches!(a.verdict("1.2.0", "0.1.0"), Verdict::Blocked { security: true, .. }));
        assert!(matches!(a.verdict("1.3.0", "0.1.0"), Verdict::Incompatible { .. }));
        assert!(a.verdict("1.3.0", "0.2.0").allowed(), "allowed once artcraftCloud is new enough");
        assert!(matches!(a.verdict("0.9.0", "0.1.0"), Verdict::Incompatible { .. }));
        assert_eq!(a.verdict("7.0.0", "0.1.0"), Verdict::Unlisted);
        a.recommended = Some("1.0.0".into());
        assert_eq!(a.target("0.1.0").unwrap().version, "1.0.0");
        a.recommended = Some("1.2.0".into());
        assert_eq!(a.target("0.1.0").unwrap().version, "1.1.0", "a blocked recommendation falls back");
    }

    #[test]
    fn validation_catches_mistakes() {
        let mut reg = Registry { schema: REGISTRY_SCHEMA, serial: 1, issued: 1, expires: 2, apps: vec![app(vec![version("1.0.0")])] };
        reg.validate().unwrap();
        reg.apps[0].versions[0].signed = "ed25519".into();
        assert!(reg.validate().is_err(), "ed25519 without keys");
        reg.apps[0].versions[0].signed = "none".into();
        reg.apps[0].versions[0].status = VersionStatus::Blocked;
        assert!(reg.validate().is_err(), "a block needs a reason");
        reg.apps[0].versions[0].reason = Some("bad".into());
        reg.validate().unwrap();
        reg.apps[0].versions[0].asset = "../x.zip".into();
        assert!(reg.validate().is_err());
        reg.apps[0].versions[0].asset = "x.zip".into();
        reg.apps[0].versions[0].url = Some("http://example.com/x.zip".into());
        assert!(reg.validate().is_err(), "plain http only to loopback");
        reg.apps[0].versions[0].url = Some("http://127.0.0.1:9000/x.zip".into());
        reg.validate().unwrap();
        assert_eq!(reg.apps[0].release_file_url(&reg.apps[0].versions[0], "SHA256SUMS.txt"), "http://127.0.0.1:9000/SHA256SUMS.txt");
        reg.apps[0].recommended = Some("2.0.0".into());
        assert!(reg.validate().is_err());
        reg.apps[0].recommended = None;
        reg.apps.push(reg.apps[0].clone());
        assert!(reg.validate().is_err(), "duplicate id");
        reg.apps.pop();
        reg.schema = 2;
        assert!(reg.validate().is_err());
    }
}
