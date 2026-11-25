//! The signed app registry: which apps and versions this artcraftCloud may install.
//!
//! Trust starts from the registry public keys built into the binary (or `ARTCRAFT_REGISTRY_KEYS`
//! for a self-hosted registry). A registry is used only when `apps.json.sig` verifies over the
//! exact bytes of `apps.json`. One fetched from the network must also be unexpired and must not
//! go back: its serial is at least that of the newest one seen, and an equal serial must be the
//! very same file. The accepted file is cached in the data volume (`registry/`), and the copy built
//! into the binary covers a first start without network.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use artcraft_cloud_types::registry::{is_loopback_http, Registry};
use artcraft_cloud_types::RegistryStatus;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// The registry's own keys: the one CI signs with and the spare kept offline for a rotation.
pub const BUILTIN_KEYS: &[&str] = &["JK1HfPRqwMAdbVCL3b+vEzN++215J7zGZZAF6wjWTcM=", "BTvARYIYd0cZiS0n54xy9KedjCqDuvtAvyhUTFXh9UE="];

pub const DEFAULT_URL: &str = "https://raw.githubusercontent.com/gheiler/artcraft-cloud/main/registry/apps.json";

/// The registry as it was when this binary was built.
pub const EMBEDDED_JSON: &[u8] = include_bytes!("../../../registry/apps.json");
pub const EMBEDDED_SIG: &str = include_str!("../../../registry/apps.json.sig");

/// The largest registry file accepted.
const MAX_BYTES: usize = 2 * 1024 * 1024;

/// The artcraftCloud version apps are checked against.
pub fn cloud_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Source {
    Network,
    Cache,
    Embedded,
}

impl Source {
    fn label(self) -> &'static str {
        match self {
            Source::Network => "network",
            Source::Cache => "cache",
            Source::Embedded => "embedded",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Loaded {
    pub registry: Registry,
    pub source: Source,
    pub sha256: String,
}

/// Where the registry comes from and which keys it must be signed with.
#[derive(Debug, Clone)]
pub struct RegistryConfig {
    pub url: String,
    pub keys: Vec<String>,
    pub custom_keys: bool,
}

impl RegistryConfig {
    pub fn from_env() -> Self {
        let url = std::env::var("ARTCRAFT_REGISTRY_URL").ok().filter(|s| !s.trim().is_empty()).unwrap_or_else(|| DEFAULT_URL.to_string());
        let custom: Vec<String> = std::env::var("ARTCRAFT_REGISTRY_KEYS").unwrap_or_default().split([',', ' ', '\n']).map(str::trim).filter(|s| !s.is_empty()).map(String::from).collect();
        let custom_keys = !custom.is_empty();
        let keys = if custom_keys { custom } else { BUILTIN_KEYS.iter().map(|k| k.to_string()).collect() };
        Self { url, keys, custom_keys }
    }

    pub fn builtin() -> Self {
        Self { url: DEFAULT_URL.into(), keys: BUILTIN_KEYS.iter().map(|k| k.to_string()).collect(), custom_keys: false }
    }

    /// Ties the cache to one source and key set: switching to a mirror with other keys starts over.
    fn trust_id(&self) -> String {
        let mut hasher = Sha256::new();
        hasher.update(self.url.as_bytes());
        for key in &self.keys {
            hasher.update(b"\n");
            hasher.update(key.as_bytes());
        }
        hex::encode(hasher.finalize())
    }
}

/// What is remembered between fetches (`registry/state.json`).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct FetchState {
    trust_id: String,
    /// The highest serial ever accepted from this source: the floor for the next one.
    serial_seen: u64,
    last_fetch: Option<i64>,
    last_attempt: Option<i64>,
    last_error: Option<String>,
}

/// Verify `sig_b64` over `bytes` with any of `keys`, then parse and check the registry.
pub fn verify(bytes: &[u8], sig_b64: &str, keys: &[String]) -> Result<Registry, String> {
    use base64::Engine;
    use ed25519_dalek::{Signature, VerifyingKey};
    let engine = base64::engine::general_purpose::STANDARD;
    let raw = engine.decode(sig_b64.trim()).map_err(|_| "the registry signature is not base64".to_string())?;
    let signature = Signature::from_slice(&raw).map_err(|_| "the registry signature is not an ed25519 signature".to_string())?;
    let signed = keys.iter().any(|key| {
        engine
            .decode(key)
            .ok()
            .and_then(|b| <[u8; 32]>::try_from(b.as_slice()).ok())
            .and_then(|b| VerifyingKey::from_bytes(&b).ok())
            .is_some_and(|key| key.verify_strict(bytes, &signature).is_ok())
    });
    if !signed {
        return Err("the registry signature matches none of the registry keys".into());
    }
    let registry: Registry = serde_json::from_slice(bytes).map_err(|err| format!("the registry does not parse: {err}"))?;
    registry.validate()?;
    Ok(registry)
}

/// Sign registry bytes with a base64 ed25519 seed (the maintainer tool and CI).
pub fn sign(bytes: &[u8], seed_b64: &str) -> Result<String, String> {
    use base64::Engine;
    use ed25519_dalek::{Signer, SigningKey};
    let engine = base64::engine::general_purpose::STANDARD;
    let seed = engine.decode(seed_b64.trim()).map_err(|_| "the signing key is not base64".to_string())?;
    let seed = <[u8; 32]>::try_from(seed.as_slice()).map_err(|_| "the signing key is not a 32-byte ed25519 seed".to_string())?;
    let key = SigningKey::from_bytes(&seed);
    Ok(engine.encode(key.sign(bytes).to_bytes()))
}

/// The public key (base64) of a base64 ed25519 seed.
pub fn public_key(seed_b64: &str) -> Result<String, String> {
    use base64::Engine;
    let engine = base64::engine::general_purpose::STANDARD;
    let seed = engine.decode(seed_b64.trim()).map_err(|_| "the signing key is not base64".to_string())?;
    let seed = <[u8; 32]>::try_from(seed.as_slice()).map_err(|_| "the signing key is not a 32-byte ed25519 seed".to_string())?;
    Ok(engine.encode(ed25519_dalek::SigningKey::from_bytes(&seed).verifying_key().to_bytes()))
}

/// May `candidate` replace what is known? Expiry and rollback rules for a fetched registry.
pub fn check_fresh(candidate: &Registry, candidate_sha: &str, floor_serial: u64, current: Option<&Loaded>, now: i64) -> Result<(), String> {
    if candidate.expired(now) {
        return Err(format!("the registry expired at {} (serial {}); refusing a stale copy", candidate.expires, candidate.serial));
    }
    if candidate.issued > now + 24 * 3600 {
        return Err("the registry is issued in the future; check this computer's clock".into());
    }
    if candidate.serial < floor_serial {
        return Err(format!("the registry went back from serial {floor_serial} to {}; refusing a rollback", candidate.serial));
    }
    if let Some(current) = current {
        if candidate.serial < current.registry.serial {
            return Err(format!("the registry went back from serial {} to {}; refusing a rollback", current.registry.serial, candidate.serial));
        }
        if candidate.serial == current.registry.serial && candidate_sha != current.sha256 && current.source != Source::Embedded {
            return Err(format!("serial {} was signed twice with different contents; refusing it", candidate.serial));
        }
    }
    Ok(())
}

fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

pub fn unix_now() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

/// When the cached copy was written and how long it is (to notice another process rewriting it).
type CacheStamp = Option<(SystemTime, u64)>;

/// The registry in use, re-read when another process (the command line) refreshed the cache.
pub struct RegistryStore {
    pub config: RegistryConfig,
    dir: PathBuf,
    current: Mutex<Option<(CacheStamp, Arc<Loaded>)>>,
    fetch_lock: tokio::sync::Mutex<()>,
}

impl RegistryStore {
    pub fn new(data: &Path, config: RegistryConfig) -> Self {
        Self { config, dir: data.join("registry"), current: Mutex::new(None), fetch_lock: tokio::sync::Mutex::new(()) }
    }

    fn json_path(&self) -> PathBuf {
        self.dir.join("apps.json")
    }

    fn sig_path(&self) -> PathBuf {
        self.dir.join("apps.json.sig")
    }

    fn state_path(&self) -> PathBuf {
        self.dir.join("state.json")
    }

    fn read_state(&self) -> FetchState {
        let state: FetchState = std::fs::read(self.state_path()).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
        if state.trust_id == self.config.trust_id() {
            state
        } else {
            FetchState { trust_id: self.config.trust_id(), ..FetchState::default() }
        }
    }

    fn write_state(&self, state: &FetchState) {
        let _ = std::fs::create_dir_all(&self.dir);
        if let Ok(text) = serde_json::to_vec_pretty(state) {
            let _ = write_atomic(&self.state_path(), &text);
        }
    }

    fn cache_stamp(&self) -> Option<(SystemTime, u64)> {
        let meta = std::fs::metadata(self.json_path()).ok()?;
        Some((meta.modified().ok()?, meta.len()))
    }

    fn load_cached(&self) -> Option<Loaded> {
        let state = self.read_state();
        if state.serial_seen == 0 {
            // Cached under another source or key set: not trusted here.
            return None;
        }
        let bytes = std::fs::read(self.json_path()).ok()?;
        let sig = std::fs::read_to_string(self.sig_path()).ok()?;
        match verify(&bytes, &sig, &self.config.keys) {
            Ok(registry) => Some(Loaded { registry, source: Source::Cache, sha256: sha256_hex(&bytes) }),
            Err(err) => {
                tracing::warn!("cached registry ignored: {err}");
                None
            }
        }
    }

    fn load_embedded(&self) -> Option<Loaded> {
        match verify(EMBEDDED_JSON, EMBEDDED_SIG, &self.config.keys) {
            Ok(registry) => Some(Loaded { registry, source: Source::Embedded, sha256: sha256_hex(EMBEDDED_JSON) }),
            Err(err) => {
                if !self.config.custom_keys {
                    tracing::warn!("the built-in registry snapshot is not usable: {err}");
                }
                None
            }
        }
    }

    /// The registry in use: the cached one or the built-in one, whichever is newer.
    pub fn current(&self) -> Option<Arc<Loaded>> {
        let stamp = self.cache_stamp();
        {
            let guard = self.current.lock().unwrap_or_else(|e| e.into_inner());
            if let Some((seen, loaded)) = guard.as_ref() {
                if *seen == stamp {
                    return Some(loaded.clone());
                }
            }
        }
        let cached = self.load_cached();
        let embedded = self.load_embedded();
        let best = match (cached, embedded) {
            (Some(c), Some(e)) => Some(if e.registry.serial > c.registry.serial { e } else { c }),
            (c, e) => c.or(e),
        }?;
        let best = Arc::new(best);
        *self.current.lock().unwrap_or_else(|e| e.into_inner()) = Some((stamp, best.clone()));
        Some(best)
    }

    pub fn status(&self) -> RegistryStatus {
        let state = self.read_state();
        let current = self.current();
        let now = unix_now();
        RegistryStatus {
            url: self.config.url.clone(),
            source: current.as_ref().map(|c| c.source.label()).unwrap_or("none").to_string(),
            serial: current.as_ref().map(|c| c.registry.serial).unwrap_or(0),
            issued: current.as_ref().map(|c| c.registry.issued).unwrap_or(0),
            expires: current.as_ref().map(|c| c.registry.expires).unwrap_or(0),
            expired: current.as_ref().is_some_and(|c| c.registry.expired(now)),
            signature_ok: current.is_some(),
            custom_keys: self.config.custom_keys,
            last_fetch: state.last_fetch,
            last_attempt: state.last_attempt,
            last_error: state.last_error,
        }
    }

    /// Fetch the registry, check it and keep it when it is newer. Errors are remembered for the
    /// Apps page; the registry in use stays as it was.
    pub async fn refresh(&self) -> Result<Arc<Loaded>, String> {
        let _guard = self.fetch_lock.lock().await;
        let now = unix_now();
        let mut state = self.read_state();
        state.last_attempt = Some(now);
        let result = self.fetch_and_accept(&mut state, now).await;
        match &result {
            Ok(_) => {
                state.last_fetch = Some(now);
                state.last_error = None;
            }
            Err(err) => {
                tracing::warn!("registry refresh: {err}");
                state.last_error = Some(err.clone());
            }
        }
        self.write_state(&state);
        result
    }

    async fn fetch_and_accept(&self, state: &mut FetchState, now: i64) -> Result<Arc<Loaded>, String> {
        let url = &self.config.url;
        if !(url.starts_with("https://") || is_loopback_http(url)) {
            return Err(format!("the registry address must be https: {url}"));
        }
        let client = reqwest::Client::builder().user_agent(USER_AGENT).timeout(Duration::from_secs(30)).redirect(reqwest::redirect::Policy::limited(3)).build().map_err(|e| e.to_string())?;
        let bytes = fetch_limited(&client, url).await?;
        let sig = fetch_limited(&client, &format!("{url}.sig")).await?;
        let sig = String::from_utf8(sig).map_err(|_| "the registry signature is not text".to_string())?;
        let registry = verify(&bytes, &sig, &self.config.keys)?;
        let sha = sha256_hex(&bytes);
        let current = self.current();
        check_fresh(&registry, &sha, state.serial_seen, current.as_deref(), now)?;
        std::fs::create_dir_all(&self.dir).map_err(|e| e.to_string())?;
        // The signature first: a reader that sees the new apps.json then also sees its signature.
        write_atomic(&self.sig_path(), sig.as_bytes())?;
        write_atomic(&self.json_path(), &bytes)?;
        state.serial_seen = state.serial_seen.max(registry.serial);
        let loaded = Arc::new(Loaded { registry, source: Source::Network, sha256: sha });
        *self.current.lock().unwrap_or_else(|e| e.into_inner()) = Some((self.cache_stamp(), loaded.clone()));
        Ok(loaded)
    }
}

pub const USER_AGENT: &str = "artcraft-cloud/0.1 (+https://github.com/gheiler/artcraft-cloud)";

async fn fetch_limited(client: &reqwest::Client, url: &str) -> Result<Vec<u8>, String> {
    let mut response = client.get(url).send().await.map_err(|err| format!("{url}: {err}"))?;
    if !response.status().is_success() {
        return Err(format!("{url}: {}", response.status()));
    }
    let mut out = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|err| err.to_string())? {
        out.extend_from_slice(&chunk);
        if out.len() > MAX_BYTES {
            return Err(format!("{url} is larger than 2 MiB"));
        }
    }
    Ok(out)
}

pub fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let tmp = path.with_extension(format!("tmp-{}", std::process::id()));
    std::fs::write(&tmp, bytes).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, path).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use artcraft_cloud_types::registry::REGISTRY_SCHEMA;

    const SEED: &str = "AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8=";

    fn reg(serial: u64, expires: i64) -> Registry {
        Registry { schema: REGISTRY_SCHEMA, serial, issued: 1, expires, apps: vec![] }
    }

    #[test]
    fn signature_must_match_a_registry_key() {
        let bytes = serde_json::to_vec(&reg(1, i64::MAX)).unwrap();
        let sig = sign(&bytes, SEED).unwrap();
        let keys = vec![public_key(SEED).unwrap()];
        assert_eq!(verify(&bytes, &sig, &keys).unwrap().serial, 1);
        let mut tampered = bytes.clone();
        let at = tampered.iter().position(|b| *b == b'1').unwrap();
        tampered[at] = b'2';
        assert!(verify(&tampered, &sig, &keys).is_err(), "changed bytes");
        assert!(verify(&bytes, &sig, &BUILTIN_KEYS.iter().map(|k| k.to_string()).collect::<Vec<_>>()).is_err(), "other key");
        assert!(verify(&bytes, "nope", &keys).is_err());
    }

    #[test]
    fn the_embedded_snapshot_verifies_with_the_builtin_keys() {
        let registry: Registry = serde_json::from_slice(EMBEDDED_JSON).expect("registry/apps.json");
        registry.validate().expect("a valid registry");
        assert!(registry.app("wordcraft").is_some());
        assert!(registry.app("mailcraft").is_some_and(|a| a.keys.ed25519.len() == 2));
        assert!(registry.app("calendarcraft").is_some_and(|a| a.keys.ed25519.len() == 2 && a.versions.iter().any(|v| v.version == "0.2.0" && v.signed == "ed25519")));
        let store = RegistryStore::new(Path::new("/nonexistent"), RegistryConfig::builtin());
        if store.load_embedded().is_none() {
            // A pull request may change the registry without the key (the registry workflow signs
            // it on merge); a release build must carry a signed copy.
            assert_eq!(std::env::var("ARTCRAFT_REGISTRY_UNSIGNED_OK").as_deref(), Ok("1"), "registry/apps.json.sig does not match registry/apps.json; sign it with `artcraft-cloud registry sign`");
        }
    }

    #[test]
    fn stale_and_rolled_back_registries_are_refused() {
        let now = 1_000_000;
        let current = Loaded { registry: reg(5, now + 100), source: Source::Cache, sha256: "a".into() };
        assert!(check_fresh(&reg(6, now + 10), "b", 5, Some(&current), now).is_ok());
        assert!(check_fresh(&reg(6, now - 1), "b", 5, Some(&current), now).unwrap_err().contains("expired"));
        assert!(check_fresh(&reg(4, now + 10), "b", 0, Some(&current), now).unwrap_err().contains("rollback"));
        assert!(check_fresh(&reg(4, now + 10), "b", 5, None, now).unwrap_err().contains("rollback"), "the floor survives a lost cache");
        assert!(check_fresh(&reg(5, now + 10), "a", 5, Some(&current), now).is_ok(), "the same file again");
        assert!(check_fresh(&reg(5, now + 10), "b", 5, Some(&current), now).unwrap_err().contains("signed twice"));
        let mut future = reg(6, now + 10_000_000);
        future.issued = now + 3 * 24 * 3600;
        assert!(check_fresh(&future, "b", 5, Some(&current), now).is_err());
    }
}
