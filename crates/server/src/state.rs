use std::fs;
use std::path::Path;
use std::sync::Arc;

use rand::RngCore;

use crate::config::Config;
use crate::db::Db;
use crate::error::Error;
use crate::oidc;
use crate::password::hash_password;
use crate::paths::{check_password_policy, display_name_of, normalize_username};
use crate::store::Store;
use crate::throttle::{BasicCache, Throttle};

#[derive(Clone)]
pub struct AppState {
    pub config: Config,
    pub db: Db,
    pub store: Store,
    pub secret: Arc<Vec<u8>>,
    pub throttle: Arc<Throttle>,
    pub basic_cache: Arc<BasicCache>,
    pub dummy_hash: Arc<String>,
}

pub struct Boot {
    pub state: AppState,
    pub generated_admin_password: Option<String>,
}

pub fn bootstrap(config: &Config) -> Result<Boot, Error> {
    oidc::reject_if_configured(config.oidc_issuer.as_deref())?;
    tracing::debug!(auth = oidc::active_method(), "local accounts");
    fs::create_dir_all(&config.data_dir)?;
    fs::create_dir_all(config.data_dir.join("files"))?;
    fs::create_dir_all(config.data_dir.join("trash"))?;
    fs::create_dir_all(config.data_dir.join("tmp"))?;
    fs::create_dir_all(config.data_dir.join("apps"))?;
    let db = Db::open(&config.data_dir.join("db.sqlite"))?;
    let secret = Arc::new(load_secret(&config.data_dir)?);
    let dummy_hash = Arc::new(hash_password(
        "artcraft-cloud-timing-pad",
        config.argon_memory_kib,
        config.argon_time,
        config.argon_parallelism,
    )?);
    let store = Store {
        db: db.clone(),
        files_root: config.data_dir.join("files"),
        trash_root: config.data_dir.join("trash"),
        tmp_root: config.data_dir.join("tmp"),
    };
    let mut generated = None;
    if db.user_count()? == 0 {
        let username = normalize_username(&config.admin_user)?;
        let (password, was_generated) = match &config.admin_password {
            Some(password) => {
                check_password_policy(password)?;
                (password.clone(), false)
            }
            None => (generate_password(), true),
        };
        let display = display_name_of(None, &username)?;
        let hash = hash_password(&password, config.argon_memory_kib, config.argon_time, config.argon_parallelism)?;
        let admin = db.insert_user(&username, &hash, "admin", &display)?;
        store.ensure_home(admin.id)?;
        if was_generated {
            generated = Some(password);
        }
    } else if let Ok(username) = normalize_username(&config.admin_user) {
        if let Some(admin) = db.user_by_username(&username)? {
            store.ensure_home(admin.id)?;
        }
    }
    let state = AppState {
        config: config.clone(),
        db,
        store,
        secret,
        throttle: Arc::new(Throttle::default()),
        basic_cache: Arc::new(BasicCache::default()),
        dummy_hash,
    };
    Ok(Boot { state, generated_admin_password: generated })
}

fn load_secret(data: &Path) -> Result<Vec<u8>, Error> {
    let path = data.join("secret.key");
    if let Ok(bytes) = fs::read(&path) {
        if bytes.len() >= 32 {
            return Ok(bytes);
        }
    }
    let mut bytes = vec![0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    fs::write(&path, &bytes)?;
    Ok(bytes)
}

fn generate_password() -> String {
    const ALPHABET: &[u8] = b"abcdefghijkmnopqrstuvwxyzABCDEFGHJKLMNPQRSTUVWXYZ23456789";
    let mut raw = [0u8; 20];
    rand::rngs::OsRng.fill_bytes(&mut raw);
    raw.into_iter().map(|byte| ALPHABET[(byte as usize) % ALPHABET.len()] as char).collect()
}

pub fn print_generated_password(password: &str) {
    eprintln!();
    eprintln!("artcraftCloud created the first admin account.");
    eprintln!("  username: (ARTCRAFT_ADMIN_USER, default admin)");
    eprintln!("  password: {password}");
    eprintln!("Change it after signing in. It is not written to the database in clear text.");
    eprintln!();
}
