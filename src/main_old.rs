mod ipc_models;
mod main_process;
mod plugin_host;

use std::pin::pin;

use clap::Parser;
use tokio::signal;
use tracing::Level;

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct RootConfig {
    #[arg(long, env = "UPSCLR_SERVER_MODE", default_value = "main")]
    mode: String,
}

#[tokio::main]
async fn main() {
    // Parse command line args and environment variables
    let config = RootConfig::parse();

    // Initialize tracing subscriber for structured logging.
    tracing_subscriber::fmt()
        .with_max_level(Level::INFO)
        .with_target(true)
        .with_file(true)
        .with_line_number(true)
        .init();

    let signal = shutdown_signal();

    match config.mode.as_str() {
        "main" => {
            tracing::info!("Starting in main process mode");
            main_process::main(signal).await;
        }
        "plugin_host" => {
            tracing::info!("Starting in plugin host mode");
            plugin_host::main(signal).await;
        }
        _ => {
            tracing::error!("Unknown mode: {}", config.mode);
            std::process::exit(1);
        }
    }
}

async fn shutdown_signal() {
    let ctrl_c = async {
        signal::ctrl_c()
            .await
            .expect("Failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        signal::unix::signal(signal::unix::SignalKind::terminate())
            .expect("Failed to install signal handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
}
