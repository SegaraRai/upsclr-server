// Entry point for the main process
// Handles initialization of the plugin manager and API server

use crate::main_process::web::{create_app, create_listener};
use crate::{main_process::plugin_manager::PluginManager, shutdown_signal::shutdown_signal};
use clap::Parser;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;
use tracing::{error, info, warn};

/// Command line arguments for upsclr-server
#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct AppConfig {
    /// Hostname/IP to bind the server to.
    #[arg(long, env = "UPSCLR_SERVER_HOST", default_value = "localhost", num_args = 0..=1, default_missing_value = "*")]
    host: String,

    /// Port number to listen on.
    #[arg(short, long, env = "UPSCLR_SERVER_PORT", default_value_t = 6795)]
    port: u16,

    /// Directory containing plugin shared libraries.
    #[arg(long, env = "UPSCLR_SERVER_PLUGINS_DIR", default_value = "plugins")]
    plugins_dir: String,
}

pub async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Parse command line args and environment variables
    let config = AppConfig::parse();

    info!("Starting main process at PID {}", std::process::id());

    // Get the executable path for spawning plugin hosts
    let executable_path = std::env::current_exe().expect("Failed to get executable path");
    info!("Executable path: {:?}", executable_path);

    // Create the plugin manager
    let plugin_manager = PluginManager::new(executable_path);
    let plugin_manager_arc = Arc::new(RwLock::new(plugin_manager));

    // Get the plugins directory
    let plugins_dir = PathBuf::from(&config.plugins_dir);
    info!("Scanning plugins directory: {:?}", plugins_dir);

    // Scan and load plugins
    match plugin_manager_arc
        .read()
        .await
        .scan_and_load_plugins(&plugins_dir)
        .await
    {
        Ok(plugins) => {
            info!("Successfully loaded {} plugins", plugins.len());
            for plugin in &plugins {
                info!("Loaded plugin: {} ({})", plugin.name, plugin.version);
            }
        }
        Err(e) => {
            error!("Failed to scan plugins directory: {:?}", e);
        }
    }

    // Create application
    let app = create_app(plugin_manager_arc.clone());

    // Create the listener
    let (addr, listener) = create_listener(&config.host, config.port)
        .await
        .inspect_err(|err| {
            error!("Failed to create listener: {}", err);
        })?;

    tracing::info!("Listening on http://{}", addr);

    // Run the server.
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal(false))
        .await
        .inspect_err(|err| {
            error!("Server error: {}", err);
        })?;

    // Exit all plugin hosts gracefully
    info!("Exiting plugin hosts...");

    let exit_all_task = async {
        plugin_manager_arc.read().await.exit_all().await;
    };

    let kill_all_task = async {
        tokio::time::sleep(Duration::from_secs(10)).await;

        warn!("Killing all plugin hosts...");
        plugin_manager_arc
            .read()
            .await
            .kill_all(Duration::from_secs(5))
            .await;
    };

    tokio::select! {
        _ = exit_all_task => {
            info!("All plugin hosts exited gracefully");
        }
        _ = kill_all_task => {
            warn!("Some plugin hosts were forcefully killed after timeout");
        }
    }

    info!("Main process exiting");

    Ok(())
}
