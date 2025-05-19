// Entry point for the main process
// Handles initialization of the plugin manager and API server

use crate::main_process::plugin_manager::PluginManager;
use std::path::PathBuf;
use std::time::Duration;
use tokio::signal;
use tracing::{error, info, warn};

pub async fn main() -> Result<(), Box<dyn std::error::Error>> {
    info!("Starting main process");

    // Get the executable path for spawning plugin hosts
    let executable_path = std::env::current_exe().expect("Failed to get executable path");
    info!("Executable path: {:?}", executable_path);

    // Create the plugin manager
    let plugin_manager = PluginManager::new(executable_path);

    // Get the plugins directory
    let plugins_dir = PathBuf::from("plugins");
    info!("Scanning plugins directory: {:?}", plugins_dir);

    // Scan and load plugins
    match plugin_manager.scan_and_load_plugins(&plugins_dir).await {
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

    // Here you would normally set up the API server, but for this example
    // we'll just wait for a shutdown signal

    // Wait for shutdown signal
    info!("Press Ctrl+C to shut down");
    signal::ctrl_c().await?;
    info!("Shutdown signal received");

    // Exit all plugin hosts gracefully
    info!("Exiting plugin hosts...");

    let exit_all_task = async {
        plugin_manager.exit_all().await;
    };

    let kill_all_task = async {
        tokio::time::sleep(Duration::from_secs(10)).await;

        warn!("Killing all plugin hosts...");
        plugin_manager.kill_all().await;
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
