mod apps;

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
    /// Create or disable accounts.
    User {
        #[command(subcommand)]
        cmd: UserCmd,
    },
}

#[derive(Subcommand)]
enum AppsCmd {
    /// Download the latest signed web builds. Pass an app id to limit the run.
    Update {
        app: Option<String>,
        /// Install this release (e.g. 0.18.0) instead of the latest; needs an app id.
        #[arg(long, requires = "app")]
        version: Option<String>,
    },
    /// Switch one app back to the version kept from the previous update.
    Rollback { app: String },
    /// Show the catalog and what is installed.
    List,
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
            if !config.skip_app_sync {
                let data = config.data_dir.clone();
                let token = config.github_token.clone();
                tokio::spawn(async move {
                    if let Err(err) = apps::sync_missing(&data, token.as_deref()).await {
                        tracing::error!("{err}");
                    }
                });
            }
            let listen = config.listen;
            serve(boot.state, listen).await.map_err(|err| err.to_string())
        }
        Command::Healthcheck => healthcheck(&config),
        Command::Apps { cmd } => match cmd {
            AppsCmd::Update { app, version } => apps::update(&config.data_dir, config.github_token.as_deref(), app.as_deref(), version.as_deref()).await,
            AppsCmd::Rollback { app } => apps::rollback(&config.data_dir, &app),
            AppsCmd::List => {
                apps::print_list(&config.data_dir);
                Ok(())
            }
        },
        Command::User { cmd } => user_cmd(&config, cmd),
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
