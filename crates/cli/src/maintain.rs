//! `artcraft-cloud registry …`: maintaining `registry/apps.json` in the artcraft-cloud repository.
//! Downloads a release, computes its checksum, checks the app's own signature, edits the file,
//! bumps the serial and signs it (with a key file or `ARTCRAFT_REGISTRY_SIGNING_KEY`, as CI does).

use std::fs;
use std::path::{Path, PathBuf};

use artcraft_cloud_server::apps::{expected_sha256, file_sha256, openpgp_fingerprints, verify_ed25519, verify_openpgp};
use artcraft_cloud_server::registry::{self, unix_now, USER_AGENT};
use artcraft_cloud_types::registry::{Registry, RegistryApp, RegistryIcon, RegistryVersion, ReleaseKeys, VersionStatus, REGISTRY_SCHEMA};

/// How long a signed registry stays valid. CI renews it monthly.
pub const VALID_DAYS: i64 = 120;
pub const KEY_ENV: &str = "ARTCRAFT_REGISTRY_SIGNING_KEY";

pub fn load(path: &Path) -> Result<Registry, String> {
    match fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(|err| format!("{}: {err}", path.display())),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(Registry { schema: REGISTRY_SCHEMA, serial: 0, issued: 0, expires: 1, apps: Vec::new() }),
        Err(err) => Err(format!("{}: {err}", path.display())),
    }
}

/// The signing key from `--key-file`, else the environment; none means "leave it for CI".
fn signing_key(key_file: Option<&Path>) -> Result<Option<String>, String> {
    if let Some(path) = key_file {
        return fs::read_to_string(path).map(|k| Some(k.trim().to_string())).map_err(|err| format!("{}: {err}", path.display()));
    }
    Ok(std::env::var(KEY_ENV).ok().map(|k| k.trim().to_string()).filter(|k| !k.is_empty()))
}

/// Bump the serial, renew the dates, validate, write and sign (when a key is at hand).
pub fn save(path: &Path, reg: &mut Registry, key_file: Option<&Path>) -> Result<(), String> {
    let now = unix_now();
    reg.schema = REGISTRY_SCHEMA;
    reg.serial += 1;
    reg.issued = now;
    reg.expires = now + VALID_DAYS * 24 * 3600;
    reg.validate()?;
    let mut text = serde_json::to_string_pretty(reg).map_err(|err| err.to_string())?;
    text.push('\n');
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|err| err.to_string())?;
    }
    fs::write(path, &text).map_err(|err| err.to_string())?;
    println!("{} serial {} (expires in {VALID_DAYS} days)", path.display(), reg.serial);
    sign_file(path, key_file, false)
}

/// Sign `path` as `path.sig`. Without a key: say so (CI signs on merge) unless `required`.
pub fn sign_file(path: &Path, key_file: Option<&Path>, required: bool) -> Result<(), String> {
    let Some(key) = signing_key(key_file)? else {
        if required {
            return Err(format!("no signing key: pass --key-file or set {KEY_ENV}"));
        }
        println!("not signed here (no --key-file or {KEY_ENV}); the registry workflow signs it on merge");
        return Ok(());
    };
    let bytes = fs::read(path).map_err(|err| err.to_string())?;
    let sig = registry::sign(&bytes, &key)?;
    let sig_path = sig_path(path);
    fs::write(&sig_path, format!("{sig}\n")).map_err(|err| err.to_string())?;
    println!("signed {} with key {}", sig_path.display(), registry::public_key(&key)?);
    Ok(())
}

fn sig_path(path: &Path) -> PathBuf {
    let mut os = path.as_os_str().to_os_string();
    os.push(".sig");
    PathBuf::from(os)
}

/// `registry verify`: the signature (built-in keys unless given), the structure, and optionally
/// that the serial grew compared with an older copy (CI on a pull request).
pub fn verify(path: &Path, keys: &[String], against: Option<&Path>, unsigned_ok: bool) -> Result<(), String> {
    let bytes = fs::read(path).map_err(|err| format!("{}: {err}", path.display()))?;
    let keys: Vec<String> = if keys.is_empty() { registry::BUILTIN_KEYS.iter().map(|k| k.to_string()).collect() } else { keys.to_vec() };
    let reg: Registry = serde_json::from_slice(&bytes).map_err(|err| err.to_string())?;
    reg.validate()?;
    match fs::read_to_string(sig_path(path)).map_err(|e| e.to_string()).and_then(|sig| registry::verify(&bytes, &sig, &keys)) {
        Ok(_) => println!("signature ok"),
        Err(err) if unsigned_ok => println!("signature not checked: {err} (the registry workflow signs on merge)"),
        Err(err) => return Err(err),
    }
    if let Some(old) = against {
        if let Ok(old_bytes) = fs::read(old) {
            let old: Registry = serde_json::from_slice(&old_bytes).map_err(|err| format!("{}: {err}", old.display()))?;
            if old_bytes != bytes && reg.serial <= old.serial {
                return Err(format!("the registry changed but its serial did not grow ({} -> {})", old.serial, reg.serial));
            }
        }
    }
    println!("{}: serial {}, {} apps, expires {}{}", path.display(), reg.serial, reg.apps.len(), reg.expires, if reg.expired(unix_now()) { " (EXPIRED)" } else { "" });
    Ok(())
}

pub fn keygen(dir: &Path, name: &str) -> Result<(), String> {
    use base64::Engine;
    let key_path = dir.join(format!("{name}.key"));
    let pub_path = dir.join(format!("{name}.pub"));
    if key_path.exists() {
        return Err(format!("{} exists; not overwriting a key", key_path.display()));
    }
    fs::create_dir_all(dir).map_err(|err| err.to_string())?;
    let mut seed = [0u8; 32];
    getrandom_fill(&mut seed)?;
    let engine = base64::engine::general_purpose::STANDARD;
    let seed_b64 = engine.encode(seed);
    write_private(&key_path, &format!("{seed_b64}\n"))?;
    let public = registry::public_key(&seed_b64)?;
    fs::write(&pub_path, format!("{public}\n")).map_err(|err| err.to_string())?;
    println!("private key: {} (keep it secret; it is the {KEY_ENV} secret)", key_path.display());
    println!("public key:  {public} ({})", pub_path.display());
    Ok(())
}

fn getrandom_fill(buf: &mut [u8]) -> Result<(), String> {
    use std::io::Read;
    fs::File::open("/dev/urandom").and_then(|mut f| f.read_exact(buf)).map_err(|err| format!("no randomness: {err}"))
}

fn write_private(path: &Path, text: &str) -> Result<(), String> {
    use std::io::Write;
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path).map_err(|err| format!("{}: {err}", path.display()))?;
    file.write_all(text.as_bytes()).map_err(|err| err.to_string())
}

pub struct NewApp {
    pub id: String,
    pub title: String,
    pub publisher: String,
    pub owner: String,
    pub repo: String,
    pub blurb: String,
    pub extensions: Vec<String>,
    pub bridge: bool,
    pub author_query: bool,
    pub chrome_query: bool,
    pub default_install: bool,
    pub ed25519: Vec<String>,
    pub openpgp: Vec<String>,
    pub icon_url: Option<String>,
    /// The icon's file, when it is not published yet (its hash is taken from here).
    pub icon_file: Option<PathBuf>,
}

pub async fn add_app(path: &Path, new: NewApp) -> Result<(), String> {
    let mut reg = load(path)?;
    if reg.app(&new.id).is_some() {
        return Err(format!("{} is already in the registry", new.id));
    }
    let icon = match new.icon_url {
        Some(url) if new.icon_file.is_some() => {
            let file = new.icon_file.as_deref().unwrap_or(Path::new(""));
            Some(RegistryIcon { url, sha256: file_sha256(file)? })
        }
        Some(url) => {
            let dir = tempdir()?;
            let file = dir.join("icon.png");
            download(&url, &file).await?;
            Some(RegistryIcon { url, sha256: file_sha256(&file)? })
        }
        None => None,
    };
    reg.apps.push(RegistryApp {
        id: new.id.clone(),
        title: new.title,
        publisher: new.publisher,
        owner: new.owner,
        repo: new.repo,
        blurb: new.blurb,
        icon,
        extensions: new.extensions.iter().map(|e| e.trim().trim_start_matches('.').to_ascii_lowercase()).filter(|e| !e.is_empty()).collect(),
        author_query: new.author_query,
        chrome_query: new.chrome_query,
        bridge: new.bridge,
        default_install: new.default_install,
        keys: ReleaseKeys { ed25519: new.ed25519, openpgp: new.openpgp.iter().map(|f| f.replace(' ', "").to_ascii_uppercase()).collect() },
        recommended: None,
        versions: Vec::new(),
    });
    println!("{} added; now `registry add-version {} <version>`", new.id, new.id);
    // Not valid until it has a version; written without signing or a serial bump.
    let mut text = serde_json::to_string_pretty(&reg).map_err(|err| err.to_string())?;
    text.push('\n');
    fs::write(path, text).map_err(|err| err.to_string())
}

pub struct NewVersion {
    pub app: String,
    pub version: String,
    pub tag: Option<String>,
    pub asset: Option<String>,
    pub url: Option<String>,
    pub recommend: bool,
    pub trust_key: bool,
    pub min_cloud: Option<String>,
    pub max_cloud: Option<String>,
    pub replace: bool,
}

pub async fn add_version(path: &Path, new: NewVersion, key_file: Option<&Path>) -> Result<(), String> {
    let mut reg = load(path)?;
    let index = reg.apps.iter().position(|a| a.id == new.app).ok_or_else(|| format!("{} is not in the registry; `registry add-app` first", new.app))?;
    let version = new.version.trim_start_matches('v').to_string();
    if reg.apps[index].version(&version).is_some() && !new.replace {
        return Err(format!("{} {version} is already listed (--replace to recompute it)", new.app));
    }
    let mut entry = RegistryVersion {
        version: version.clone(),
        tag: new.tag.unwrap_or_else(|| format!("v{version}")),
        asset: new.asset.unwrap_or_else(|| format!("{}-web-{version}.zip", new.app)),
        url: new.url,
        sha256: String::new(),
        size: 0,
        signed: "none".into(),
        status: VersionStatus::Ok,
        reason: None,
        security: false,
        min_cloud: new.min_cloud,
        max_cloud: new.max_cloud,
    };
    let dir = tempdir()?;
    let app = reg.apps[index].clone();
    let zip = dir.join(&entry.asset);
    let zip_url = app.download_url(&entry);
    println!("downloading {zip_url}");
    download(&zip_url, &zip).await?;
    entry.sha256 = file_sha256(&zip)?;
    entry.size = fs::metadata(&zip).map(|m| m.len()).unwrap_or(0);
    // The app's own signature over SHA256SUMS.txt, when its release has one.
    let sums_path = dir.join("SHA256SUMS.txt");
    let have_sums = download(&app.release_file_url(&entry, "SHA256SUMS.txt"), &sums_path).await.is_ok();
    let mut new_prints = Vec::new();
    if have_sums {
        let sums = fs::read_to_string(&sums_path).map_err(|err| err.to_string())?;
        let listed = expected_sha256(&sums, &entry.asset)?;
        if listed != entry.sha256 {
            return Err(format!("SHA256SUMS.txt names {listed}, the download is {}", entry.sha256));
        }
        let ed = dir.join("SHA256SUMS.txt.sig");
        let asc = dir.join("SHA256SUMS.txt.asc");
        let key = dir.join("release-signing-key.asc");
        if download(&app.release_file_url(&entry, "SHA256SUMS.txt.sig"), &ed).await.is_ok() {
            if app.keys.ed25519.is_empty() {
                return Err(format!("the release is signed with ed25519 but the registry lists no key for {}; add it with --ed25519-key on add-app (or edit keys.ed25519), checked against the app's own published key", app.id));
            }
            let sig = fs::read_to_string(&ed).map_err(|err| err.to_string())?;
            verify_ed25519(&app.keys.ed25519, sums.as_bytes(), &sig)?;
            entry.signed = "ed25519".into();
        } else if download(&app.release_file_url(&entry, "SHA256SUMS.txt.asc"), &asc).await.is_ok() {
            download(&app.release_file_url(&entry, "release-signing-key.asc"), &key).await?;
            let prints = openpgp_fingerprints(&key)?;
            let pinned: Vec<String> = app.keys.openpgp.clone();
            let trusted: Vec<String> = if pinned.is_empty() || new.trust_key {
                if !new.trust_key {
                    return Err(format!(
                        "{} {version} is signed with OpenPGP key {}, which the registry does not pin yet. Check that fingerprint against the publisher, then rerun with --trust-key",
                        app.id,
                        prints.join(", ")
                    ));
                }
                new_prints = prints.clone();
                pinned.iter().cloned().chain(prints).collect()
            } else {
                pinned
            };
            verify_openpgp(&key, &sums_path, &asc, &trusted)?;
            entry.signed = "openpgp".into();
        } else if !app.keys.ed25519.is_empty() || !app.keys.openpgp.is_empty() {
            return Err(format!("{} pins release keys but {version} is not signed; refusing", app.id));
        }
    } else if !app.keys.ed25519.is_empty() || !app.keys.openpgp.is_empty() {
        return Err(format!("{} pins release keys but {version} has no SHA256SUMS.txt; refusing", app.id));
    }
    println!("{} {version}: sha256 {} ({} bytes), release signature: {}", app.id, entry.sha256, entry.size, entry.signed);
    let app = &mut reg.apps[index];
    for fpr in new_prints {
        if !app.keys.openpgp.contains(&fpr) {
            app.keys.openpgp.push(fpr);
        }
    }
    app.versions.retain(|v| v.version != version);
    app.versions.push(entry);
    app.versions.sort_by(|a, b| artcraft_cloud_types::registry::compare_versions(&a.version, &b.version));
    if new.recommend {
        app.recommended = Some(version);
    }
    let _ = fs::remove_dir_all(&dir);
    save(path, &mut reg, key_file)
}

/// What `registry block` / `registry set-status` changes on one version.
pub struct StatusChange {
    pub status: VersionStatus,
    pub reason: Option<String>,
    pub security: bool,
    pub min_cloud: Option<String>,
    pub max_cloud: Option<String>,
}

pub fn set_status(path: &Path, app: &str, version: &str, change: StatusChange, key_file: Option<&Path>) -> Result<(), String> {
    let StatusChange { status, reason, security, min_cloud, max_cloud } = change;
    let version = version.trim_start_matches('v');
    let mut reg = load(path)?;
    let entry = reg.apps.iter_mut().find(|a| a.id == app).ok_or_else(|| format!("{app} is not in the registry"))?;
    let v = entry.versions.iter_mut().find(|v| v.version == version).ok_or_else(|| format!("{app} {version} is not listed"))?;
    v.status = status;
    v.reason = reason.filter(|r| !r.trim().is_empty());
    v.security = security && status == VersionStatus::Blocked;
    if min_cloud.is_some() {
        v.min_cloud = min_cloud;
    }
    if max_cloud.is_some() {
        v.max_cloud = max_cloud;
    }
    if status == VersionStatus::Blocked && entry.recommended.as_deref() == Some(version) {
        entry.recommended = None;
        println!("{app} {version} was the recommended version; no version is recommended now (the newest allowed one is offered)");
    }
    save(path, &mut reg, key_file)
}

pub fn recommend(path: &Path, app: &str, version: Option<&str>, key_file: Option<&Path>) -> Result<(), String> {
    let mut reg = load(path)?;
    let entry = reg.apps.iter_mut().find(|a| a.id == app).ok_or_else(|| format!("{app} is not in the registry"))?;
    entry.recommended = version.map(|v| v.trim_start_matches('v').to_string());
    save(path, &mut reg, key_file)
}

pub fn renew(path: &Path, key_file: Option<&Path>) -> Result<(), String> {
    let mut reg = load(path)?;
    save(path, &mut reg, key_file)
}

fn tempdir() -> Result<PathBuf, String> {
    let dir = std::env::temp_dir().join(format!("artcraft-registry-{}-{}", std::process::id(), unix_now()));
    fs::create_dir_all(&dir).map_err(|err| err.to_string())?;
    Ok(dir)
}

async fn download(url: &str, dest: &Path) -> Result<(), String> {
    let client = reqwest::Client::builder().user_agent(USER_AGENT).build().map_err(|err| err.to_string())?;
    let mut response = client.get(url).send().await.map_err(|err| format!("{url}: {err}"))?;
    if !response.status().is_success() {
        return Err(format!("{url}: {}", response.status()));
    }
    let mut file = tokio::fs::File::create(dest).await.map_err(|err| err.to_string())?;
    while let Some(chunk) = response.chunk().await.map_err(|err| err.to_string())? {
        tokio::io::AsyncWriteExt::write_all(&mut file, &chunk).await.map_err(|err| err.to_string())?;
    }
    tokio::io::AsyncWriteExt::flush(&mut file).await.map_err(|err| err.to_string())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_sign_and_status_changes_bump_the_serial() {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().to_path_buf();
        keygen(&dir, "test").unwrap();
        assert!(keygen(&dir, "test").is_err(), "a key is never overwritten");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(fs::metadata(dir.join("test.key")).unwrap().permissions().mode() & 0o777, 0o600);
        }
        let public = fs::read_to_string(dir.join("test.pub")).unwrap().trim().to_string();
        let key = dir.join("test.key");
        let file = dir.join("apps.json");
        let old = dir.join("old.json");
        fs::copy(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../registry/apps.json"), &file).unwrap();
        fs::copy(&file, &old).unwrap();
        let serial = load(&file).unwrap().serial;

        let change = StatusChange { status: VersionStatus::Blocked, reason: Some("broken".into()), security: true, min_cloud: None, max_cloud: None };
        set_status(&file, "wordcraft", "v0.18.0", change, Some(&key)).unwrap();
        let reg = load(&file).unwrap();
        assert_eq!(reg.serial, serial + 1);
        let v = reg.app("wordcraft").unwrap().version("0.18.0").unwrap();
        assert!(v.status == VersionStatus::Blocked && v.security && v.reason.as_deref() == Some("broken"));
        verify(&file, std::slice::from_ref(&public), Some(&old), false).unwrap();
        assert!(verify(&file, &[], None, false).is_err(), "signed with the test key, not the built-in ones");

        // Editing by hand without a new serial is caught; so is a missing signature.
        fs::copy(&old, &file).unwrap();
        let mut text = fs::read_to_string(&file).unwrap();
        text = text.replacen("\"Word processor\"", "\"Word processor!\"", 1);
        fs::write(&file, text).unwrap();
        assert!(verify(&file, std::slice::from_ref(&public), Some(&old), true).unwrap_err().contains("serial"));
        assert!(verify(&file, std::slice::from_ref(&public), None, false).is_err());
    }
}
