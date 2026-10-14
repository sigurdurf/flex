//! Flex headless media server — bootstrap.

#![forbid(unsafe_code)]

mod api;
mod auth;
mod config;
mod db;
mod error;
mod model;
mod playback;
mod probe;
mod scanner;
mod subtitles;
mod transcode;
mod user;

use std::net::SocketAddr;
use std::sync::Arc;

use clap::Parser;
use flex_protocol::UserRole;
use tracing_subscriber::EnvFilter;

use crate::api::AppState;
use crate::config::Cli;
use crate::playback::SessionManager;

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

/// Create the initial administrator from the environment, if configured.
///
/// Only runs when the `users` table is empty. Never invents a default password.
async fn bootstrap_admin(pool: &sqlx::SqlitePool) -> anyhow::Result<()> {
    if db::count_users(pool).await? > 0 {
        return Ok(());
    }

    let username = std::env::var("FLEX_ADMIN_USER")
        .ok()
        .filter(|value| !value.is_empty());
    let password = std::env::var("FLEX_ADMIN_PASSWORD")
        .ok()
        .filter(|value| !value.is_empty());

    match (username, password) {
        (Some(username), Some(password)) => {
            let hash = auth::hash_password(&password)
                .map_err(|e| anyhow::anyhow!("failed to hash bootstrap password: {e}"))?;
            let admin = db::create_user(pool, username.trim(), &hash, UserRole::Admin).await?;
            tracing::info!(username = %admin.username, "created bootstrap administrator");
        }
        _ => {
            tracing::warn!(
                "no users configured; create one with `flex-server user add <name> --admin` \
                 or set FLEX_ADMIN_USER and FLEX_ADMIN_PASSWORD"
            );
        }
    }

    Ok(())
}

/// Reclaim idle/expired playback sessions and their files on a fixed cadence.
async fn gc_task(sessions: Arc<SessionManager>) {
    let mut ticker = tokio::time::interval(playback::GC_INTERVAL);
    ticker.tick().await;
    loop {
        ticker.tick().await;
        let removed = sessions.gc(playback::IDLE_TIMEOUT).await;
        if removed > 0 {
            tracing::info!(removed, "reclaimed idle playback sessions");
        }
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    init_tracing();

    let cli = Cli::parse();

    if let Some(command) = cli.command.clone() {
        return user::run(command, &cli).await;
    }

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
    bootstrap_admin(&pool).await?;

    let cache_dir = config.cache_path();
    tokio::fs::create_dir_all(&cache_dir)
        .await
        .map_err(|e| anyhow::anyhow!("failed to create cache dir {}: {e}", cache_dir.display()))?;

    let transcode_caps = transcode::detect().await;
    if transcode_caps.available {
        tracing::info!(
            hwaccels = ?transcode_caps.hwaccels,
            hw_encoders = ?transcode_caps.hw_encoders,
            h264_encoder = %transcode_caps.h264_encoder,
            hevc_encoder = %transcode_caps.hevc_encoder,
            "detected ffmpeg transcode capabilities"
        );
    } else {
        tracing::warn!("ffmpeg unavailable; remux and transcode requests will be refused");
    }

    let sessions = Arc::new(SessionManager::new(
        cache_dir.clone(),
        config.max_transcode_sessions,
    ));
    if let Err(err) = sessions.sweep().await {
        tracing::warn!(error = %err, "failed to clear stale playback session directories");
    }

    let state = AppState {
        pool: pool.clone(),
        config: Arc::new(config.clone()),
        rate_limiter: Arc::new(auth::LoginRateLimiter::new()),
        sessions: sessions.clone(),
        transcode: Arc::new(transcode_caps),
        subtitles: Arc::new(subtitles::SubtitleRegistry::from_config(&config)),
    };

    tokio::spawn(gc_task(sessions));

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

    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown_signal())
    .await?;

    Ok(())
}
