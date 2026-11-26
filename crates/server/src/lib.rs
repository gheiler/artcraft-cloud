//! artcraftCloud server: one process for accounts, files, sharing, WebDAV and
//! the static Crafting App builds.

pub mod apps;
mod capability;
mod config;
mod db;
mod error;
mod http;
mod manifest;
pub mod sideload;
mod oidc;
mod password;
mod paths;
pub mod registry;
mod state;
mod sniff;
mod static_files;
mod store;
mod stalwart;
mod throttle;
mod vault;
mod webdav;

pub use config::Config;
pub use db::{Account, Db};
pub use error::Error;
pub use manifest::{current_dir, read_manifest, version_dir, write_manifest, InstalledApp, Manifest, SideloadMeta, SideloadSource};
pub use password::hash_password;
pub use paths::{check_password_policy, normalize_username};
pub use state::{bootstrap, print_generated_password, AppState, Boot};

pub fn router(state: AppState) -> axum::Router {
    http::router(state)
}

pub async fn serve(state: AppState, listen: std::net::SocketAddr) -> Result<(), std::io::Error> {
    let listener = tokio::net::TcpListener::bind(listen).await?;
    tracing::info!(%listen, "artcraftCloud listening");
    // Mailbox password changes that could not reach the mail server are retried from here.
    tokio::spawn(http::mail::retry_loop(state.clone()));
    // The app registry, its blocks, auto-updates, and the default apps of a new install.
    tokio::spawn(http::apps::apps_loop(state.clone()));
    axum::serve(listener, router(state)).with_graceful_shutdown(shutdown_signal()).await
}

/// Connect the mail server outside the web UI (`artcraft-cloud mail connect`); returns what is
/// stored, without the secret.
pub async fn mail_connect(state: &AppState, update: artcraft_cloud_types::MailConfigUpdate) -> Result<artcraft_cloud_types::MailConfig, String> {
    http::mail::connect(state, &update).await.map_err(|err| err.message)
}

/// Fetch the registry, apply its blocks and fetch icons (`artcraft-cloud apps registry refresh`).
pub async fn registry_refresh(state: &AppState) -> Result<(), String> {
    http::apps::refresh_and_enforce(state).await
}

/// The Apps page's view (`artcraft-cloud apps list` / `apps registry show`).
pub fn apps_view(state: &AppState) -> artcraft_cloud_types::AdminApps {
    http::apps::admin_view(state)
}

/// Check the stored mail connection (`artcraft-cloud mail test`).
pub async fn mail_test(state: &AppState) -> Result<artcraft_cloud_types::MailTest, String> {
    http::mail::test_stored(state).await.map_err(|err| err.message)
}

async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let terminate = async {
        if let Ok(mut signal) = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            signal.recv().await;
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();
    tokio::select! {
        _ = ctrl_c => {}
        _ = terminate => {}
    }
    tracing::info!("shutting down");
}

#[cfg(test)]
mod tests;
