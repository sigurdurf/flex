//! Flex headless media server — bootstrap.

#![forbid(unsafe_code)]

mod api;
mod config;
mod db;
mod error;
mod model;
mod probe;
mod scanner;

use std::sync::Arc;

use clap::Parser;
use tracing_subscriber::EnvFilter;

use crate::api::AppState;
use crate::config::Cli;

fn init_tracing() {
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("flex_server=info,tower_http=info"));
    tracing_subscriber::fmt().with_env_filter(filter).init();
}

async fn shutdown_signal() {
    if let Err(err) = tokio::signal::ctrl_c().await {
        tracing::error!(error = %err, "failed to listen for shutdown signal");
    }
    tracing::info!("shutdown signal received");
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    init_tracing();

    let cli = Cli::parse();
    let (config, loaded) = config::load(&cli)?;
    tracing::info!(
        config = %cli.config.display(),
        loaded,
        host = %config.host,
        port = config.port,
        database = %config.database,
        libraries = config.libraries.len(),
        "starting flex-server"
    );

    let pool = db::init_pool(&config.database).await?;
    let state = AppState {
        pool: pool.clone(),
        config: Arc::new(config.clone()),
    };

    if cli.scan_on_start {
        let report = scanner::scan(&pool, &config).await;
        if report.errors.is_empty() {
            tracing::info!(?report, "startup scan complete");
        } else {
            tracing::warn!(?report, "startup scan complete with errors");
        }
    }

    let app = api::router(state);
    let addr = format!("{}:{}", config.host, config.port);
    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .map_err(|e| anyhow::anyhow!("failed to bind {addr}: {e}"))?;
    tracing::info!("listening on http://{}", listener.local_addr()?);

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;

    Ok(())
}
