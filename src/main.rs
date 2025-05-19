use upsclr_server::ipc::{BootstrapClient, BootstrapHost};
use futures::StreamExt;
use std::time::Duration;
use tarpc::{context, server::Channel};
use tracing::Level;

async fn main_wrapper() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_max_level(Level::DEBUG)
        .with_target(true)
        .with_file(true)
        .with_line_number(true)
        .init();

    // Check if we are running in plugin host mode
    let args: Vec<String> = std::env::args().collect();
    if args.contains(&"--mode=plugin".to_string()) {
        let ipc_name = args
            .iter()
            .find(|&arg| arg.starts_with("--ipc="))
            .and_then(|arg| arg.split('=').nth(1))
            .expect("IPC name must be provided in plugin host mode");

        return example_rpc_host(ipc_name).await;
    }

    // Run the main process example
    example_main_process().await
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("Failed to create Tokio runtime")
        .block_on(async {
            let result = main_wrapper().await;

            let result = result.inspect_err(|err| {
                tracing::error!("Error in main process: {:?}", err);
            });

            // Create a timer to force the main process to exit after a timeout
            // This is needed since `IpcOneShotServer::accept()` blocks indefinitely
            std::thread::spawn(|| {
                std::thread::sleep(Duration::from_secs(10));
                tracing::warn!("Main process timed out, exiting...");
                std::process::exit(1);
            });

            result
        })
}

#[tarpc::service]
pub trait PidService {
    async fn get_pid() -> u32;
}

// Server implementation
#[derive(Clone)]
pub struct PidServer;

impl PidService for PidServer {
    async fn get_pid(self, _ctx: context::Context) -> u32 {
        tracing::debug!("Server handling get_pid request");
        let pid = std::process::id();
        tracing::debug!("Server returning PID: {}", pid);
        pid
    }
}

// Main Process Example
async fn example_main_process() -> Result<(), Box<dyn std::error::Error>> {
    // Create bootstrap server and wait for RPC host to connect
    let (bootstrap_host, bootstrap_name) = BootstrapHost::new()?;
    tracing::info!("Tell RPC host to connect to bootstrap: {}", bootstrap_name);

    let (transport, _bootstrap_host) = bootstrap_host.accept(Duration::from_secs(20)).await?;
    tracing::info!("RPC host connected to bootstrap");

    // Configure tarpc client with longer timeout
    let client_config = tarpc::client::Config::default();

    tracing::debug!(
        "Creating new RPC client with configuration: {:?}",
        client_config
    );
    // Create the service-specific client
    let new_client = PidServiceClient::new(client_config, transport);
    tracing::info!("New RPC client created");

    // Make RPC calls to the RPC host
    tracing::debug!("Sending get_pid request to RPC host");

    // Set a shorter timeout for debugging
    let ctx = context::current();

    let client = new_client.client;
    let dispatch = new_client.dispatch;
    let dispatch_handle = tokio::spawn(async move {
        if let Err(e) = dispatch.await {
            println!("RPC client dispatch error: {}", e);
            Err(e)
        } else {
            Ok(())
        }
    });

    match client.get_pid(ctx).await {
        Ok(pid) => {
            tracing::debug!("Received successful response from RPC host");
            println!("RPC host PID: {}", pid);
        }
        Err(e) => {
            tracing::error!("Error calling get_pid on RPC host: {:?}", e);
            return Err(Box::new(std::io::Error::new(
                std::io::ErrorKind::Other,
                format!("RPC call failed: {:?}", e),
            )));
        }
    }

    dispatch_handle.abort();

    Ok(())
}

// RPC Host Example
async fn example_rpc_host(bootstrap_name: &str) -> Result<(), Box<dyn std::error::Error>> {
    // Connect to the main process
    let (transport, _bootstrap_client) = BootstrapClient::connect(bootstrap_name)?;
    println!(
        "Connected to main process via bootstrap: {}",
        bootstrap_name
    );

    // Configure server with longer timeouts
    tracing::debug!("Creating server to handle requests");
    let server = tarpc::server::BaseChannel::with_defaults(transport);

    tracing::debug!("Setting up server task with PidServer implementation");
    let server_task = tokio::spawn(
        server
            .execute(PidServer.serve())
            // Handle all requests concurrently
            .for_each(|response| async move {
                tracing::debug!("Got a request, spawning handler task");
                tokio::spawn(response);
            }),
    );

    tracing::debug!("Server task started, waiting for shutdown signal");

    // Wait for some signal to shutdown
    let ctrl_c = tokio::signal::ctrl_c();

    tokio::select! {
        _ = ctrl_c => {
            tracing::info!("RPC host: Ctrl-C received, shutting down");
        }
        _ = server_task => {
            tracing::info!("RPC host: Client disconnected, shutting down");
        }
    }

    Ok(())
}
