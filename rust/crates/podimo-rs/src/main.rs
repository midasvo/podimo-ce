use std::net::SocketAddr;

use anyhow::Context;
use podimo_rs::{app, config::Config, telemetry};
use tokio::net::TcpListener;
use tokio::signal;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Load .env if present, then env overrides — same precedence as the Python service.
    let _ = dotenvy::dotenv();
    let config = Config::from_env().context("loading configuration")?;

    telemetry::init(config.debug);
    config.log_startup();

    let addr: SocketAddr = config
        .bind_host
        .parse()
        .with_context(|| format!("parsing PODIMO_BIND_HOST={}", config.bind_host))?;

    let listener = TcpListener::bind(addr)
        .await
        .with_context(|| format!("binding {addr}"))?;
    tracing::info!(target: "podimo", "listening on {}", listener.local_addr()?);

    let app = app(config.into_shared()).await?;

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .context("server crashed")?;

    Ok(())
}

async fn shutdown_signal() {
    let ctrl_c = async {
        signal::ctrl_c().await.expect("install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        signal::unix::signal(signal::unix::SignalKind::terminate())
            .expect("install SIGTERM handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }

    tracing::info!(target: "podimo", "shutting down");
}
