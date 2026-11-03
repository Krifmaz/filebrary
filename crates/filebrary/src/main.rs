//! The `filebrary` command-line entry point.

use std::path::PathBuf;

use anyhow::Context;
use clap::Parser;
use filebrary::{AppState, config::Config, crypto, db};

/// A small, fast, self-hosted file manager.
#[derive(Parser, Debug)]
#[command(version, about)]
struct Cli {
    /// Path to a TOML configuration file.
    #[arg(short, long, env = "FILEBRARY_CONFIG", global = true)]
    config: Option<PathBuf>,
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(clap::Subcommand, Debug)]
enum Command {
    /// Run the server (the default).
    Serve,
    /// Probe `/healthz` on the configured address; exit 0 if healthy.
    /// Used by the Docker HEALTHCHECK (the image has no curl).
    Healthcheck,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let config = Config::load(cli.config.as_deref())?;
    if let Some(Command::Healthcheck) = cli.command {
        return healthcheck(config.listen).await;
    }
    filebrary::ops::logging::init(config.log_format);
    // One TLS crypto provider for the whole process (notifications, FTPS):
    // ring, which needs no system libraries.
    let _ = rustls::crypto::ring::default_provider().install_default();

    filebrary::ops::datadir::ensure_writable(&config.data_dir)?;
    let pool = db::open(&config.data_dir).await?;
    let secret = crypto::load_or_create_secret(&config.data_dir)?;
    let listen = config.listen;
    let state = AppState::new(config, pool, secret)?;
    // Subscribe the content indexer before the file indexer starts publishing.
    filebrary::content::start(&state);
    filebrary::spawn_maintenance(state.clone());
    filebrary::identity::install(&state);
    filebrary::identity::spawn_maintenance(state.clone());
    filebrary::auth::bootstrap_admin(&state)
        .await
        .map_err(|e| anyhow::anyhow!("creating the first administrator: {e}"))?;
    let ftp = filebrary::ops::ftp::spawn(&state, shutdown_signal())?;
    state.search.start(&state);
    if let Some(p) = &state.previews {
        p.cache().spawn_janitor(std::time::Duration::from_secs(600));
    }

    let listener = tokio::net::TcpListener::bind(listen)
        .await
        .with_context(|| format!("binding {listen}"))?;
    tracing::info!(%listen, version = env!("CARGO_PKG_VERSION"), "filebrary listening");
    axum::serve(
        listener,
        filebrary::app(state).into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown_signal())
    .await?;
    if let Some(ftp) = ftp {
        // The FTP server saw the same signal; give it its grace period.
        let _ = tokio::time::timeout(std::time::Duration::from_secs(6), ftp).await;
    }
    Ok(())
}

async fn healthcheck(mut addr: std::net::SocketAddr) -> anyhow::Result<()> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    if addr.ip().is_unspecified() {
        addr.set_ip(if addr.is_ipv4() {
            std::net::Ipv4Addr::LOCALHOST.into()
        } else {
            std::net::Ipv6Addr::LOCALHOST.into()
        });
    }
    let probe = async {
        let mut s = tokio::net::TcpStream::connect(addr).await?;
        s.write_all(b"GET /healthz HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
            .await?;
        let mut buf = [0u8; 12];
        s.read_exact(&mut buf).await?;
        anyhow::Ok(buf.starts_with(b"HTTP/1.1 200"))
    };
    match tokio::time::timeout(std::time::Duration::from_secs(5), probe).await {
        Ok(Ok(true)) => Ok(()),
        Ok(Ok(false)) => anyhow::bail!("unhealthy response from {addr}"),
        Ok(Err(e)) => Err(e.context(format!("probing {addr}"))),
        Err(_) => anyhow::bail!("timed out probing {addr}"),
    }
}

async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let term = async {
        if let Ok(mut s) = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        {
            s.recv().await;
        }
    };
    #[cfg(not(unix))]
    let term = std::future::pending::<()>();
    // Both arms are Send + Sync, so the FTP server can wait on this too.
    tokio::select! {
        () = ctrl_c => {},
        () = term => {},
    }
    tracing::info!("shutting down");
}
