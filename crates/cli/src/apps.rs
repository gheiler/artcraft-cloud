//! Download, verify and install Crafting App web builds into the data volume.

use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::{Component, Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use artcraft_cloud_server::{read_manifest, version_dir, write_manifest, InstalledApp};
use artcraft_cloud_types::{app_by_id, catalog};
use fs2::FileExt;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use zip::ZipArchive;

const MAX_UNPACKED: u64 = 800 * 1024 * 1024;
const USER_AGENT: &str = "artcraft-cloud/0.1 (+https://github.com/gheiler/artcraft-cloud)";

pub async fn sync_missing(data: &Path, token: Option<&str>) -> Result<(), String> {
    let manifest = read_manifest(data);
    let mut errors = Vec::new();
    for app in catalog() {
        let present = manifest.apps.get(app.id).is_some_and(|row| version_dir(data, app.id, &row.version).join("index.html").is_file());
        if present {
            continue;
        }
        tracing::info!(app = app.id, "downloading web build");
        if let Err(err) = update_one(data, token, app.id, None).await {
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

pub async fn update(data: &Path, token: Option<&str>, only: Option<&str>, version: Option<&str>) -> Result<(), String> {
    if let Some(id) = only {
        if app_by_id(id).is_none() {
            return Err(format!("unknown app {id}"));
        }
        return update_one(data, token, id, version).await;
    }
    let mut errors = Vec::new();
    for app in catalog() {
        if let Err(err) = update_one(data, token, app.id, None).await {
            errors.push(format!("{}: {err}", app.id));
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.join("; "))
    }
}

pub fn rollback(data: &Path, app_id: &str) -> Result<(), String> {
    if app_by_id(app_id).is_none() {
        return Err(format!("unknown app {app_id}"));
    }
    with_lock(data, || {
        let mut manifest = read_manifest(data);
        let row = manifest.apps.get_mut(app_id).ok_or_else(|| format!("{app_id} is not installed"))?;
        let previous = row.previous.clone().ok_or_else(|| format!("{app_id} has no previous version"))?;
        if !version_dir(data, app_id, &previous).join("index.html").is_file() {
            return Err(format!("previous files for {app_id} {previous} are missing"));
        }
        let current = row.version.clone();
        row.version = previous;
        row.previous = Some(current);
        write_manifest(data, &manifest)
    })
}

pub fn print_list(data: &Path) {
    let manifest = read_manifest(data);
    println!("{:<12} {:<10} {:<10} {:<12} extensions", "app", "version", "previous", "signature");
    for app in catalog() {
        match manifest.apps.get(app.id) {
            Some(row) => println!(
                "{:<12} {:<10} {:<10} {:<12} {}",
                app.id,
                row.version,
                row.previous.as_deref().unwrap_or("-"),
                row.signature,
                app.extensions.join(",")
            ),
            None => println!("{:<12} {:<10} {:<10} {:<12} {}", app.id, "-", "-", "absent", app.extensions.join(",")),
        }
    }
}

async fn update_one(data: &Path, token: Option<&str>, app_id: &str, pinned: Option<&str>) -> Result<(), String> {
    let app = app_by_id(app_id).ok_or_else(|| format!("unknown app {app_id}"))?;
    let client = reqwest::Client::builder().user_agent(USER_AGENT).build().map_err(|err| err.to_string())?;
    let release = github_release(&client, app.owner, app.repo, token, pinned).await?;
    let version = release.tag_name.trim_start_matches('v').to_string();
    let installed = read_manifest(data).apps.get(app_id).map(|row| row.version.clone());
    if pinned.is_none() && installed.as_deref() == Some(version.as_str()) && version_dir(data, app_id, &version).join("index.html").is_file() {
        println!("{app_id} {version} is already the latest");
        return Ok(());
    }
    let zip_name = format!("{app_id}-web-{version}.zip");
    let zip_url = asset_url(&release, &zip_name)?;
    let sums_url = asset_url(&release, "SHA256SUMS.txt")?;
    let sums_sig = release.assets.iter().find(|asset| asset.name == "SHA256SUMS.txt.asc").map(|asset| asset.browser_download_url.clone());
    let key_url = release.assets.iter().find(|asset| asset.name == "release-signing-key.asc").map(|asset| asset.browser_download_url.clone());

    let scratch = data.join("tmp").join(format!("fetch-{app_id}-{version}"));
    let _ = fs::remove_dir_all(&scratch);
    fs::create_dir_all(&scratch).map_err(|err| err.to_string())?;
    let zip_path = scratch.join(&zip_name);
    let sums_path = scratch.join("SHA256SUMS.txt");
    download(&client, token, &zip_url, &zip_path).await?;
    download(&client, token, &sums_url, &sums_path).await?;
    let sums = fs::read_to_string(&sums_path).map_err(|err| err.to_string())?;
    let expected = expected_sha256(&sums, &zip_name)?;
    let actual = file_sha256(&zip_path)?;
    if !actual.eq_ignore_ascii_case(&expected) {
        return Err(format!("{zip_name} sha256 {actual} != {expected}"));
    }
    let ed_sig = release.assets.iter().find(|asset| asset.name == "SHA256SUMS.txt.sig").map(|asset| asset.browser_download_url.clone());
    let signature = if !app.release_keys.is_empty() {
        // The app names its own release keys, so an unsigned release is refused rather than trusted.
        let sig_url = ed_sig.ok_or_else(|| format!("{app_id} {version} has no SHA256SUMS.txt.sig; refusing to install"))?;
        let sig_path = scratch.join("SHA256SUMS.txt.sig");
        download(&client, token, &sig_url, &sig_path).await?;
        let sig = fs::read_to_string(&sig_path).map_err(|err| err.to_string())?;
        verify_ed25519(app.release_keys, sums.as_bytes(), &sig)?;
        "verified"
    } else if let (Some(sig_url), Some(key)) = (sums_sig.as_deref(), key_url.as_deref()) {
        let sig_path = scratch.join("SHA256SUMS.txt.asc");
        let key_path = scratch.join("release-signing-key.asc");
        download(&client, token, sig_url, &sig_path).await?;
        download(&client, token, key, &key_path).await?;
        verify_detached(&key_path, &sums_path, &sig_path)?;
        "verified"
    } else if sums_sig.is_some() || key_url.is_some() {
        return Err(format!("{app_id} published a partial signature; refusing to install"));
    } else {
        "unsigned"
    };
    install_zip(data, app_id, &version, &zip_path, &actual, signature)?;
    let _ = fs::remove_dir_all(&scratch);
    println!("installed {app_id} {version} ({signature})");
    Ok(())
}

pub fn install_zip(data: &Path, app_id: &str, version: &str, zip_path: &Path, sha256: &str, signature: &str) -> Result<(), String> {
    if app_by_id(app_id).is_none() {
        return Err(format!("unknown app {app_id}"));
    }
    let actual = file_sha256(zip_path)?;
    if !actual.eq_ignore_ascii_case(sha256) {
        return Err(format!("sha256 mismatch for {app_id}"));
    }
    if app_by_id(app_id).is_some_and(|app| app.bridge) {
        archive_has_bridge(zip_path, app_id)?;
    } else {
        archive_names_are_safe(zip_path)?;
    }
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
        let previous = manifest.apps.get(app_id).and_then(|row| {
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
        manifest.apps.insert(
            app_id.to_string(),
            InstalledApp {
                version: version.to_string(),
                previous,
                sha256: actual,
                signature: signature.to_string(),
                installed_at: unix_now(),
            },
        );
        write_manifest(data, &manifest)
    })
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
pub fn verify_ed25519(keys: &[&str], message: &[u8], signature_b64: &str) -> Result<(), String> {
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

fn file_sha256(path: &Path) -> Result<String, String> {
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

pub fn verify_detached(key_armor: &Path, message: &Path, signature: &Path) -> Result<(), String> {
    let home = message.parent().unwrap_or(Path::new(".")).join(format!("gpg-{}", std::process::id()));
    let _ = fs::remove_dir_all(&home);
    fs::create_dir_all(&home).map_err(|err| err.to_string())?;
    let gpg = |args: &[&Path]| {
        let mut command = Command::new("gpg");
        command.arg("--batch").arg("--no-default-keyring").arg("--homedir").arg(&home);
        for arg in args {
            command.arg(arg);
        }
        command.output()
    };
    let imported = Command::new("gpg")
        .arg("--batch")
        .arg("--no-default-keyring")
        .arg("--homedir")
        .arg(&home)
        .arg("--import")
        .arg(key_armor)
        .output()
        .map_err(|_| "gpg is not installed; refusing a signed app update".to_string())?;
    if !imported.status.success() {
        return Err(format!("gpg import failed: {}", String::from_utf8_lossy(&imported.stderr)));
    }
    let verified = gpg(&[Path::new("--verify"), signature, message]).map_err(|err| err.to_string())?;
    let _ = fs::remove_dir_all(&home);
    if verified.status.success() {
        Ok(())
    } else {
        Err(format!("signature rejected: {}", String::from_utf8_lossy(&verified.stderr)))
    }
}

fn with_lock<T>(data: &Path, body: impl FnOnce() -> Result<T, String>) -> Result<T, String> {
    fs::create_dir_all(data.join("apps")).map_err(|err| err.to_string())?;
    let lock_path = data.join("apps").join("manifest.lock");
    let lock = fs::OpenOptions::new().create(true).write(true).truncate(false).open(&lock_path).map_err(|err| err.to_string())?;
    lock.lock_exclusive().map_err(|err| err.to_string())?;
    let result = body();
    drop(lock);
    result
}

fn unix_now() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

#[derive(Deserialize)]
struct Release {
    tag_name: String,
    assets: Vec<Asset>,
}

#[derive(Deserialize)]
struct Asset {
    name: String,
    browser_download_url: String,
}

async fn github_release(client: &reqwest::Client, owner: &str, repo: &str, token: Option<&str>, version: Option<&str>) -> Result<Release, String> {
    let url = match version {
        Some(version) => format!("https://api.github.com/repos/{owner}/{repo}/releases/tags/v{}", version.trim_start_matches('v')),
        None => format!("https://api.github.com/repos/{owner}/{repo}/releases/latest"),
    };
    let mut request = client.get(url).header("accept", "application/vnd.github+json");
    if let Some(token) = token {
        request = request.bearer_auth(token);
    }
    let response = request.send().await.map_err(|err| err.to_string())?;
    if !response.status().is_success() {
        return Err(format!("GitHub releases for {repo}: {}", response.status()));
    }
    response.json().await.map_err(|err| err.to_string())
}

fn asset_url(release: &Release, name: &str) -> Result<String, String> {
    release.assets.iter().find(|asset| asset.name == name).map(|asset| asset.browser_download_url.clone()).ok_or_else(|| format!("release {} has no {name}", release.tag_name))
}

async fn download(client: &reqwest::Client, token: Option<&str>, url: &str, dest: &Path) -> Result<(), String> {
    let mut request = client.get(url);
    if url.contains("api.github.com") {
        if let Some(token) = token {
            request = request.bearer_auth(token);
        }
    }
    let mut response = request.send().await.map_err(|err| err.to_string())?;
    if !response.status().is_success() {
        return Err(format!("download {url}: {}", response.status()));
    }
    let mut file = tokio::fs::File::create(dest).await.map_err(|err| err.to_string())?;
    while let Some(chunk) = response.chunk().await.map_err(|err| err.to_string())? {
        tokio::io::AsyncWriteExt::write_all(&mut file, &chunk).await.map_err(|err| err.to_string())?;
    }
    file.sync_all().await.map_err(|err| err.to_string())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use zip::write::SimpleFileOptions;
    use zip::ZipWriter;

    #[test]
    fn mailcraft_release_signature() {
        // SHA256SUMS.txt and its signature from MailCraft v0.15.0, checked by hand on 2026-11-24.
        let sums = include_bytes!("../tests/fixtures/mailcraft-0.15.0-SHA256SUMS.txt");
        let sig = include_str!("../tests/fixtures/mailcraft-0.15.0-SHA256SUMS.txt.sig");
        let keys = artcraft_cloud_types::app_by_id("mailcraft").unwrap().release_keys;
        verify_ed25519(keys, sums, sig).unwrap();
        let mut tampered = sums.to_vec();
        tampered[0] ^= 1;
        assert!(verify_ed25519(keys, &tampered, sig).is_err());
        assert!(verify_ed25519(&keys[1..], sums, sig).is_err(), "the spare key did not sign it");
        assert!(verify_ed25519(keys, sums, "not base64!").is_err());
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
        install_zip(data, "wordcraft", "0.0.1", &zip1, &sha1, "unsigned").unwrap();
        assert!(version_dir(data, "wordcraft", "0.0.1").join("index.html").is_file());
        assert!(version_dir(data, "wordcraft", "0.0.1").join("index.html.gz").is_file());

        let zip2 = data.join("two.zip");
        write_app_zip(&zip2, "wordcraft", "two");
        let sha2 = file_sha256(&zip2).unwrap();
        install_zip(data, "wordcraft", "0.0.2", &zip2, &sha2, "unsigned").unwrap();
        let manifest = read_manifest(data);
        let row = manifest.apps.get("wordcraft").unwrap();
        assert_eq!(row.version, "0.0.2");
        assert_eq!(row.previous.as_deref(), Some("0.0.1"));
        rollback(data, "wordcraft").unwrap();
        assert_eq!(read_manifest(data).apps.get("wordcraft").unwrap().version, "0.0.1");

        let evil = data.join("evil.zip");
        let mut writer = ZipWriter::new(File::create(&evil).unwrap());
        writer.start_file("../evil.txt", SimpleFileOptions::default()).unwrap();
        writer.write_all(b"wordcraft:ready host=parent").unwrap();
        writer.finish().unwrap();
        let sha = file_sha256(&evil).unwrap();
        assert!(install_zip(data, "wordcraft", "9.9.9", &evil, &sha, "unsigned").is_err());
    }

    #[test]
    fn gpg_detached_signature_round_trip() {
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
        let message = dir.path().join("msg");
        fs::write(&message, b"checksums").unwrap();
        let sig = dir.path().join("msg.asc");
        let signed = Command::new("gpg").arg("--batch").arg("--homedir").arg(&home).arg("--armor").arg("--detach-sign").arg("--output").arg(&sig).arg(&message).status().unwrap();
        assert!(signed.success());
        verify_detached(&key, &message, &sig).unwrap();
        fs::write(&message, b"tampered").unwrap();
        assert!(verify_detached(&key, &message, &sig).is_err());
    }

    fn write_app_zip(path: &Path, app: &str, marker: &str) {
        let mut writer = ZipWriter::new(File::create(path).unwrap());
        writer.start_file(format!("{app}-web/index.html"), SimpleFileOptions::default()).unwrap();
        writer.write_all(format!("<html>{app}:ready host=parent {marker}</html>").as_bytes()).unwrap();
        writer.start_file(format!("{app}-web/app.js"), SimpleFileOptions::default()).unwrap();
        writer.write_all(b"console.log('ok')").unwrap();
        writer.finish().unwrap();
    }
}
