//! artcraftCloud server: one process for accounts, files, sharing, WebDAV and
//! the static Crafting App builds.

mod config;
mod db;
mod error;
mod http;
mod manifest;
mod oidc;
mod password;
mod paths;
mod state;
mod static_files;
mod store;
mod throttle;
mod webdav;

pub use config::Config;
pub use db::{Account, Db};
pub use error::Error;
pub use manifest::{current_dir, read_manifest, version_dir, write_manifest, InstalledApp, Manifest};
pub use password::hash_password;
pub use paths::{check_password_policy, normalize_username};
pub use state::{bootstrap, print_generated_password, AppState, Boot};

pub fn router(state: AppState) -> axum::Router {
    http::router(state)
}

pub async fn serve(state: AppState, listen: std::net::SocketAddr) -> Result<(), std::io::Error> {
    let listener = tokio::net::TcpListener::bind(listen).await?;
    tracing::info!(%listen, "artcraftCloud listening");
    axum::serve(listener, router(state)).with_graceful_shutdown(shutdown_signal()).await
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
