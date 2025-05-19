// Entry point for running the plugin host process
// Handles receiving the bootstrap client name and connecting to the main process

use super::plugin_host_impl::PluginHostServiceImpl;
use crate::plugin_host_service::PluginHostService;
use crate::{ipc::BootstrapClient, shutdown_signal::shutdown_signal};
use futures::StreamExt;
use tarpc::server::Channel;
use tracing::{debug, info};

pub async fn main(bootstrap_name: &str) -> Result<(), Box<dyn std::error::Error>> {
    info!("Starting plugin host process");
    info!(
        "Connecting to main process via bootstrap: {}",
        bootstrap_name
    );

    // Connect to the main process
    let (transport, _bootstrap_client) = BootstrapClient::connect(bootstrap_name)?;
    info!("Connected to main process");

    // Configure server
    let server = tarpc::server::BaseChannel::with_defaults(transport);

    // Create the plugin host service implementation
    let service = PluginHostServiceImpl::new();

    // Start the server task
    let server_task = tokio::spawn(
        server
            .execute(service.serve())
            // Handle all requests concurrently
            .for_each(|response| async move {
                debug!("Handling a plugin host service request");
                tokio::spawn(response);
            }),
    );

    info!("Plugin host service started, waiting for shutdown signal");

    let shutdown_signal_task = shutdown_signal(true);

    // Wait for ctrl-c or client disconnection
    tokio::select! {
        _ = shutdown_signal_task => {
            info!("Plugin host received shutdown signal, shutting down");
        }
        _ = server_task => {
            info!("Main process disconnected, shutting down plugin host");
        }
    }

    info!("Plugin host process exiting");
    Ok(())
}
