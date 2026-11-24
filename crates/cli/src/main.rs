mod apps;
mod maintain;

use std::process::ExitCode;

use artcraft_cloud_server::{bootstrap, print_generated_password, serve, Config};
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "artcraft-cloud", version, about = "Self-hosted ArtCraft office")]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Listen for HTTP (the default).
    Serve,
    /// Exit 0 when /healthz on ARTCRAFT_LISTEN answers.
    Healthcheck,
    /// Install and roll back Crafting App web builds.
    Apps {
        #[command(subcommand)]
        cmd: AppsCmd,
    },
    /// Maintain the app registry file (`registry/apps.json` in the artcraft-cloud repository).
    Registry {
        #[command(subcommand)]
        cmd: RegistryCmd,
    },
    /// Create or disable accounts.
    User {
        #[command(subcommand)]
        cmd: UserCmd,
    },
    /// Connect the Stalwart mail server (the same as Mail in the web UI).
    Mail {
        #[command(subcommand)]
        cmd: MailCmd,
    },
}

#[derive(Subcommand)]
enum MailCmd {
    /// Store the server's address and an API key. With --admin, the admin password is read from
    /// standard input, used once to create an API key, and only the key is stored.
    Connect {
        /// Stalwart's HTTP address, e.g. http://stalwart:8080.
        #[arg(long)]
        url: String,
        /// Read the API key from this environment variable.
        #[arg(long, conflicts_with = "admin")]
        api_key_env: Option<String>,
        /// A Stalwart admin account (e.g. admin@example.com); the password comes on stdin.
        #[arg(long)]
        admin: Option<String>,
        /// With --admin: store the admin login instead of creating an API key (not recommended).
        #[arg(long, requires = "admin")]
        keep_admin_login: bool,
        /// The host name people put in other mail apps (default: the domain's MX).
        #[arg(long, default_value = "")]
        public_host: String,
    },
    /// Check that the stored credential can manage mailboxes.
    Test,
}

#[derive(Subcommand)]
enum AppsCmd {
    /// Install what the registry recommends: every installed app (and the default ones missing),
    /// or one app. Only versions the signed registry allows are installed.
    Update {
        app: Option<String>,
        /// Install this allowed version (e.g. 0.18.0) instead; needs an app id.
        #[arg(long, requires = "app")]
        version: Option<String>,
    },
    /// Switch one app back to the version kept from the previous update.
    Rollback { app: String },
    /// Install a web build from a local archive (a build of your own, or one copied in by
    /// hand). The app must be in the registry and still carry the host bridge when it has one.
    Install {
        app: String,
        /// The `<app>-web-<version>.zip` archive.
        zip: std::path::PathBuf,
        /// The version to record (e.g. 0.2.0).
        #[arg(long)]
        version: String,
    },
    /// Delete an app's files (documents stay); it is not installed again by itself.
    Remove { app: String },
    /// Hide an app from everyone and stop serving it.
    Disable { app: String },
    /// Serve a disabled app again.
    Enable { app: String },
    /// Turn auto-update on or off for one app.
    AutoUpdate {
        app: String,
        #[arg(value_parser = ["on", "off"])]
        state: String,
    },
    /// The registry's apps and what is installed.
    List,
    /// The signed app registry this server uses.
    Registry {
        #[command(subcommand)]
        cmd: AppsRegistryCmd,
    },
}

#[derive(Subcommand)]
enum AppsRegistryCmd {
    /// Fetch and verify the registry now, and apply its blocks.
    Refresh,
    /// Where the registry comes from, the one in use, and its apps.
    Show,
}

#[derive(Subcommand)]
enum RegistryCmd {
    /// Make an ed25519 key pair for signing the registry (NAME.key, NAME.pub).
    Keygen {
        #[arg(long)]
        out_dir: std::path::PathBuf,
        #[arg(long, default_value = "registry")]
        name: String,
    },
    /// List a new app (then add a version).
    AddApp {
        id: String,
        #[arg(long)]
        title: String,
        #[arg(long)]
        publisher: String,
        /// GitHub owner of the releases.
        #[arg(long)]
        owner: String,
        #[arg(long)]
        repo: String,
        #[arg(long, default_value = "")]
        blurb: String,
        /// File types it opens, comma separated (e.g. docx,odt).
        #[arg(long, value_delimiter = ',')]
        ext: Vec<String>,
        /// The web build speaks the craft-web-host bridge (opens and saves drive files).
        #[arg(long)]
        bridge: bool,
        #[arg(long)]
        author_query: bool,
        #[arg(long)]
        chrome_query: bool,
        /// Install it by itself on new installs.
        #[arg(long)]
        default_install: bool,
        /// An ed25519 public key (base64) that signs the app's SHA256SUMS.txt; repeatable.
        #[arg(long)]
        ed25519_key: Vec<String>,
        /// An OpenPGP fingerprint that signs the app's SHA256SUMS.txt.asc; repeatable.
        #[arg(long)]
        openpgp_key: Vec<String>,
        /// An https URL of a PNG icon (its SHA-256 is pinned).
        #[arg(long)]
        icon_url: Option<String>,
        /// The icon file, when the URL is not published yet.
        #[arg(long, requires = "icon_url")]
        icon_file: Option<std::path::PathBuf>,
        #[arg(long, default_value = "registry/apps.json")]
        file: std::path::PathBuf,
    },
    /// Download a release, check it and list it as allowed.
    AddVersion {
        app: String,
        version: String,
        /// Release tag (default v<version>).
        #[arg(long)]
        tag: Option<String>,
        /// Archive name (default <app>-web-<version>.zip).
        #[arg(long)]
        asset: Option<String>,
        /// Download address, when not the GitHub release asset.
        #[arg(long)]
        url: Option<String>,
        /// Make it the recommended version.
        #[arg(long)]
        recommend: bool,
        /// Pin the OpenPGP key the release is signed with (after checking its fingerprint).
        #[arg(long)]
        trust_key: bool,
        #[arg(long)]
        min_cloud: Option<String>,
        #[arg(long)]
        max_cloud: Option<String>,
        /// Recompute a version that is already listed.
        #[arg(long)]
        replace: bool,
        #[command(flatten)]
        common: RegistryFile,
    },
    /// Block a version (compromised, broken); installs running it are warned, and for --security
    /// rolled back or disabled.
    Block {
        app: String,
        version: String,
        #[arg(long)]
        reason: String,
        #[arg(long)]
        security: bool,
        #[command(flatten)]
        common: RegistryFile,
    },
    /// Set a version's status: ok, blocked or incompatible (with --min-cloud / --max-cloud).
    SetStatus {
        app: String,
        version: String,
        #[arg(value_parser = ["ok", "blocked", "incompatible"])]
        status: String,
        #[arg(long)]
        reason: Option<String>,
        #[arg(long)]
        security: bool,
        #[arg(long)]
        min_cloud: Option<String>,
        #[arg(long)]
        max_cloud: Option<String>,
        #[command(flatten)]
        common: RegistryFile,
    },
    /// Set (or with no version, clear) the recommended version.
    Recommend {
        app: String,
        version: Option<String>,
        #[command(flatten)]
        common: RegistryFile,
    },
    /// Bump the serial and the expiry date and sign (the monthly CI run).
    Renew {
        #[command(flatten)]
        common: RegistryFile,
    },
    /// Sign the file as it is (apps.json.sig).
    Sign {
        #[command(flatten)]
        common: RegistryFile,
    },
    /// Check the signature (built-in keys unless --key) and the structure.
    Verify {
        #[arg(long, default_value = "registry/apps.json")]
        file: std::path::PathBuf,
        /// A public key to check against instead of the built-in ones; repeatable.
        #[arg(long = "key")]
        keys: Vec<String>,
        /// An older copy: the serial must have grown if the file changed.
        #[arg(long)]
        against: Option<std::path::PathBuf>,
        /// Do not fail on a missing or stale signature (a pull request before CI signs).
        #[arg(long)]
        unsigned_ok: bool,
    },
}

#[derive(clap::Args)]
struct RegistryFile {
    /// The registry file.
    #[arg(long, default_value = "registry/apps.json")]
    file: std::path::PathBuf,
    /// The private key (base64 ed25519 seed); default ARTCRAFT_REGISTRY_SIGNING_KEY.
    #[arg(long)]
    key_file: Option<std::path::PathBuf>,
}

#[derive(Subcommand)]
enum UserCmd {
    Create {
        #[arg(long)]
        username: String,
        #[arg(long)]
        password: String,
        #[arg(long)]
        admin: bool,
    },
    Disable {
        username: String,
    },
    Enable {
        username: String,
    },
}

#[tokio::main]
async fn main() -> ExitCode {
    tracing_subscriber::fmt().with_ansi(std::io::IsTerminal::is_terminal(&std::io::stdout())).with_env_filter(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into())).init();
    let cli = Cli::parse();
    let command = cli.command.unwrap_or(Command::Serve);
    match run(command).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("error: {err}");
            ExitCode::FAILURE
        }
    }
}

async fn run(command: Command) -> Result<(), String> {
    let config = Config::from_env().map_err(|err| err.to_string())?;
    match command {
        Command::Serve => {
            let boot = bootstrap(&config).map_err(|err| err.to_string())?;
            if let Some(password) = &boot.generated_admin_password {
                print_generated_password(password);
            }
            let listen = config.listen;
            serve(boot.state, listen).await.map_err(|err| err.to_string())
        }
        Command::Healthcheck => healthcheck(&config),
        Command::Apps { cmd } => match cmd {
            AppsCmd::Update { app, version } => apps::update(&config, app.as_deref(), version.as_deref()).await,
            AppsCmd::Rollback { app } => apps::rollback(&config, &app),
            AppsCmd::Install { app, zip, version } => apps::install_local(&config, &app, &version, &zip),
            AppsCmd::Remove { app } => apps::remove(&config, &app),
            AppsCmd::Disable { app } => apps::settings(&config, &app, None, Some(true)),
            AppsCmd::Enable { app } => apps::settings(&config, &app, None, Some(false)),
            AppsCmd::AutoUpdate { app, state } => apps::settings(&config, &app, Some(state == "on"), None),
            AppsCmd::List => apps::list(&config),
            AppsCmd::Registry { cmd: AppsRegistryCmd::Refresh } => apps::registry_refresh(&config).await,
            AppsCmd::Registry { cmd: AppsRegistryCmd::Show } => apps::registry_show(&config),
        },
        Command::Registry { cmd } => registry_cmd(cmd).await,
        Command::User { cmd } => user_cmd(&config, cmd),
        Command::Mail { cmd } => mail_cmd(&config, cmd).await,
    }
}

async fn registry_cmd(cmd: RegistryCmd) -> Result<(), String> {
    use artcraft_cloud_types::registry::VersionStatus;
    match cmd {
        RegistryCmd::Keygen { out_dir, name } => maintain::keygen(&out_dir, &name),
        RegistryCmd::AddApp { id, title, publisher, owner, repo, blurb, ext, bridge, author_query, chrome_query, default_install, ed25519_key, openpgp_key, icon_url, icon_file, file } => {
            let new = maintain::NewApp { id, title, publisher, owner, repo, blurb, extensions: ext, bridge, author_query, chrome_query, default_install, ed25519: ed25519_key, openpgp: openpgp_key, icon_url, icon_file };
            maintain::add_app(&file, new).await
        }
        RegistryCmd::AddVersion { app, version, tag, asset, url, recommend, trust_key, min_cloud, max_cloud, replace, common } => {
            let new = maintain::NewVersion { app, version, tag, asset, url, recommend, trust_key, min_cloud, max_cloud, replace };
            maintain::add_version(&common.file, new, common.key_file.as_deref()).await
        }
        RegistryCmd::Block { app, version, reason, security, common } => {
            let change = maintain::StatusChange { status: VersionStatus::Blocked, reason: Some(reason), security, min_cloud: None, max_cloud: None };
            maintain::set_status(&common.file, &app, &version, change, common.key_file.as_deref())
        }
        RegistryCmd::SetStatus { app, version, status, reason, security, min_cloud, max_cloud, common } => {
            let status = match status.as_str() {
                "blocked" => VersionStatus::Blocked,
                "incompatible" => VersionStatus::Incompatible,
                _ => VersionStatus::Ok,
            };
            let change = maintain::StatusChange { status, reason, security, min_cloud, max_cloud };
            maintain::set_status(&common.file, &app, &version, change, common.key_file.as_deref())
        }
        RegistryCmd::Recommend { app, version, common } => maintain::recommend(&common.file, &app, version.as_deref(), common.key_file.as_deref()),
        RegistryCmd::Renew { common } => maintain::renew(&common.file, common.key_file.as_deref()),
        RegistryCmd::Sign { common } => maintain::sign_file(&common.file, common.key_file.as_deref(), true),
        RegistryCmd::Verify { file, keys, against, unsigned_ok } => maintain::verify(&file, &keys, against.as_deref(), unsigned_ok),
    }
}

async fn mail_cmd(config: &Config, cmd: MailCmd) -> Result<(), String> {
    let boot = bootstrap(config).map_err(|err| err.to_string())?;
    if let Some(password) = &boot.generated_admin_password {
        print_generated_password(password);
    }
    match cmd {
        MailCmd::Connect { url, api_key_env, admin, keep_admin_login, public_host } => {
            let update = match (api_key_env, admin) {
                (Some(var), None) => {
                    let key = std::env::var(&var).map_err(|_| format!("{var} is not set"))?;
                    artcraft_cloud_types::MailConfigUpdate { base_url: url, auth: "bearer".into(), secret: Some(key), public_host, ..Default::default() }
                }
                (None, Some(admin)) => {
                    let mut password = String::new();
                    std::io::stdin().read_line(&mut password).map_err(|err| err.to_string())?;
                    let password = password.trim_end_matches(['\r', '\n']).to_string();
                    if password.is_empty() {
                        return Err("pipe the admin password on standard input".into());
                    }
                    artcraft_cloud_types::MailConfigUpdate {
                        base_url: url,
                        auth: "basic".into(),
                        username: admin,
                        secret: Some(password),
                        create_api_key: !keep_admin_login,
                        public_host,
                    }
                }
                _ => return Err("pass --api-key-env VAR or --admin USER (password on stdin)".into()),
            };
            let stored = artcraft_cloud_server::mail_connect(&boot.state, update).await?;
            println!("mail server {} stored ({})", stored.base_url, if stored.auth == "bearer" { "API key" } else { "admin login" });
            let test = artcraft_cloud_server::mail_test(&boot.state).await?;
            println!("{}", test.message);
            if test.ok {
                Ok(())
            } else {
                Err("the credential cannot manage mailboxes yet".into())
            }
        }
        MailCmd::Test => {
            let test = artcraft_cloud_server::mail_test(&boot.state).await?;
            println!("{}", test.message);
            if test.ok {
                Ok(())
            } else {
                Err("not ready".into())
            }
        }
    }
}

fn user_cmd(config: &Config, cmd: UserCmd) -> Result<(), String> {
    let boot = bootstrap(config).map_err(|err| err.to_string())?;
    if let Some(password) = &boot.generated_admin_password {
        print_generated_password(password);
    }
    let db = &boot.state.db;
    match cmd {
        UserCmd::Create { username, password, admin } => {
            let username = artcraft_cloud_server::normalize_username(&username).map_err(|err| err.to_string())?;
            artcraft_cloud_server::check_password_policy(&password).map_err(|err| err.to_string())?;
            let hash = artcraft_cloud_server::hash_password(&password, config.argon_memory_kib, config.argon_time, config.argon_parallelism).map_err(|err| err.to_string())?;
            let role = if admin { "admin" } else { "user" };
            let account = db.insert_user(&username, &hash, role, &username).map_err(|err| err.to_string())?;
            boot.state.store.ensure_home(account.id).map_err(|err| err.to_string())?;
            println!("created {role} {username}");
        }
        UserCmd::Disable { username } => set_user_enabled(db, &username, false)?,
        UserCmd::Enable { username } => set_user_enabled(db, &username, true)?,
    }
    Ok(())
}

fn set_user_enabled(db: &artcraft_cloud_server::Db, username: &str, enabled: bool) -> Result<(), String> {
    let username = artcraft_cloud_server::normalize_username(username).map_err(|err| err.to_string())?;
    let account = db.user_by_username(&username).map_err(|err| err.to_string())?.ok_or_else(|| format!("no such user: {username}"))?;
    db.set_disabled(account.id, !enabled).map_err(|err| err.to_string())?;
    println!("{} {username}", if enabled { "enabled" } else { "disabled" });
    Ok(())
}

fn healthcheck(config: &Config) -> Result<(), String> {
    use std::io::{Read, Write};
    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], config.listen.port()));
    let mut stream = std::net::TcpStream::connect_timeout(&addr, std::time::Duration::from_secs(3)).map_err(|err| err.to_string())?;
    stream.set_read_timeout(Some(std::time::Duration::from_secs(3))).ok();
    stream.write_all(b"GET /healthz HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n").map_err(|err| err.to_string())?;
    let mut buf = String::new();
    stream.read_to_string(&mut buf).map_err(|err| err.to_string())?;
    if buf.starts_with("HTTP/1.1 200") || buf.starts_with("HTTP/1.0 200") {
        Ok(())
    } else {
        Err(format!("unhealthy: {}", buf.lines().next().unwrap_or("no response")))
    }
}
