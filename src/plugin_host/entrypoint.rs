use crate::ipc_models::{Bootstrap, PidService, PidServiceRequest, PidServiceResponse};
use base64::prelude::{BASE64_URL_SAFE_NO_PAD, Engine as _};
use clap::Parser;
use futures::{SinkExt, StreamExt};
use ipc_channel::ipc::{self, IpcReceiver, IpcSender};
use std::future::Future;
use tarpc::{
    ClientMessage, Response,
    server::{self, Channel},
};
use tokio::task::JoinHandle;
use tracing::{error, info};

// Custom server implementation
#[derive(Clone)]
pub struct PidServer;

impl PidService for PidServer {
    async fn get_pid(self, _: tarpc::context::Context) -> u32 {
        // Return the current process ID
        std::process::id()
    }
}

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct PluginHostConfig {
    #[arg(long, env = "UPSCLR_SERVER_MODE", default_value = "")]
    mode: String,

    #[arg(long, env = "UPSCLR_SERVER_IPC")]
    ipc: String,
}

pub async fn main(shutdown_signal: impl Future<Output = ()> + Send + 'static) {
    info!(
        "Plugin host process started with PID: {}",
        std::process::id()
    );

    let config = PluginHostConfig::parse();

    println!("Plugin host configuration: {:?}", config);

    let bootstrap = IpcSender::<Bootstrap>::connect(config.ipc).unwrap();

    let (request_tx, request_rx) = ipc::channel::<ClientMessage<PidServiceRequest>>().unwrap();
    let (response_tx, response_rx) = ipc::channel::<Response<PidServiceResponse>>().unwrap();

    bootstrap
        .send(Bootstrap {
            request_tx,
            response_rx,
        })
        .expect("Failed to send bootstrap message");

    let (client_transport, server_transport) = tarpc::transport::channel::unbounded();

    // Create a cancellation token that we can use to signal the server to stop
    let (shutdown_token, shutdown_listener) = tokio::sync::broadcast::channel::<()>(1);

    // Clone the shutdown token for each spawned task that needs to be shut down
    let mut server_shutdown = shutdown_listener.resubscribe();

    // Create server with graceful shutdown capability
    let server = server::BaseChannel::with_defaults(server_transport);
    let server_future = server
        .execute(PidServer.serve())
        // Handle all requests concurrently.
        .for_each(|response| async move {
            tokio::spawn(response);
        });

    // Wrap the server future with a cancellation token
    let mut server_task = tokio::spawn(async move {
        tokio::select! {
            _ = server_future => {
                info!("Server future completed naturally");
            }
            _ = server_shutdown.recv() => {
                info!("Server shutdown requested, stopping gracefully");
            }
        }
    });

    let mut request_rx = request_rx.to_stream();
    let mut client_transport = client_transport.fuse();

    let mut shutdown_signal = tokio::spawn(shutdown_signal);

    loop {
        tokio::select! {
            message = request_rx.next() => {
                match message {
                    Some(Ok(message)) => {
                        info!("Received request: {:?}", message);
                        let _ = client_transport.send(message).await.inspect_err(|err| {
                            error!("Error sending response: {}", err);
                        });
                    }
                    Some(Err(err)) => {
                        error!("Error receiving IPC message: {}", err);
                        // Break the loop when IPC channel is closed
                        info!("IPC channel closed, exiting plugin host");
                        break;
                    }
                    None => {
                        info!("IPC request channel closed, exiting plugin host");
                        break;
                    }
                }
            }
            message = client_transport.next() => {
                match message {
                    Some(Ok(message)) => {
                        info!("Received response: {:?}", message);
                        let _ = response_tx.send(message).inspect_err(|err| {
                            error!("Error sending IPC message: {}", err);
                        });
                    }
                    Some(Err(err)) => {
                        error!("Error receiving client message: {}", err);
                        // Break the loop when client transport is closed
                        info!("Client transport closed, exiting plugin host");
                        break;
                    }
                    None => {
                        info!("Client transport closed, exiting plugin host");
                        break;
                    }
                }
            }
            _ = &mut shutdown_signal => {
                info!("Shutdown signal received, exiting plugin host");
                break;
            }
            _ = &mut server_task => {
                info!("Server task exited, shutting down plugin host");
                break;
            }
        }
    }

    info!("Beginning graceful shutdown...");

    // Signal server to shut down gracefully
    let _ = shutdown_token.send(());

    // Close client transport first
    info!("Closing client transport...");
    if let Err(e) = client_transport.close().await {
        error!("Error closing client transport: {}", e);
    } else {
        info!("Client transport closed successfully");
    }

    // Wait for server task to complete with timeout
    info!("Waiting for server task to complete...");
    match tokio::time::timeout(std::time::Duration::from_secs(5), &mut server_task).await {
        Ok(res) => {
            if let Err(e) = res {
                error!("Server task exited with error: {}", e);
            } else {
                info!("Server task completed successfully");
            }
        }
        Err(_) => {
            info!("Server task did not complete within timeout, forcefully aborting");
            server_task.abort();
        }
    }

    info!("Plugin host shutting down");
}
