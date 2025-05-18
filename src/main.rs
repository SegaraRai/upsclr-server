mod rpc_ipc_framework2;

use futures::StreamExt;
use rpc_ipc_framework2::{
    BootstrapConfirmation, MainProcessBootstrap, MainProcessClientFactory, RpcClientConfig,
    RpcHostBootstrapClient, RpcHostServerFactory, RpcServerConfig,
};
use tarpc::{context, server::Channel};
use tracing::{Level, info};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
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

#[tarpc::service]
pub trait PidService {
    async fn get_pid() -> u32;
}

// Server implementation
#[derive(Clone)]
pub struct PidServer;

impl PidService for PidServer {
    async fn get_pid(self, _ctx: context::Context) -> u32 {
        std::process::id()
    }
}

// Main Process Example
async fn example_main_process() -> Result<(), Box<dyn std::error::Error>> {
    // Create bootstrap server and wait for RPC host to connect
    let (bootstrap, request_tx, response_rx) =
        MainProcessBootstrap::<PidServiceRequest, PidServiceResponse>::new().await?;

    println!(
        "Tell RPC host to connect to bootstrap: {}",
        bootstrap.bootstrap_name()
    );

    // Create a client that talks to the RPC host with bridge
    let (client, bridge_task) = MainProcessClientFactory::create_client(
        request_tx,
        response_rx,
        RpcClientConfig::default(),
        PidServiceClient::new,
    )?;

    // Make RPC calls to the RPC host
    let pid = client.inner_client().get_pid(context::current()).await?;
    println!("RPC host PID: {}", pid);

    // Graceful shutdown
    client.shutdown().await;
    bootstrap.shutdown().await;
    bridge_task.await?;

    Ok(())
}

// RPC Host Example
async fn example_rpc_host(bootstrap_name: &str) -> Result<(), Box<dyn std::error::Error>> {
    // Connect to the main process
    let (request_rx, response_tx, bootstrap_context) =
        RpcHostBootstrapClient::connect::<PidServiceRequest, PidServiceResponse>(bootstrap_name)
            .await?;

    // Create a server to handle requests from the main process
    let (server, bridge_task) = RpcHostServerFactory::create_server(
        PidServer,
        |server, channel| {
            channel
                .execute(server.serve())
                .for_each(|response| async move {
                    tokio::spawn(response);
                })
        },
        request_rx,
        response_tx,
        RpcServerConfig::default(),
    )
    .await?;

    // Set up a task to monitor bootstrap confirmation messages
    let bootstrap_monitor = tokio::spawn(async move {
        let mut confirmation_rx = bootstrap_context.confirmation_rx.to_stream();
        while let Some(Ok(confirmation)) = confirmation_rx.next().await {
            match confirmation {
                BootstrapConfirmation::Ready => {
                    info!("RPC host: Received ready confirmation from main process");
                }
                BootstrapConfirmation::Shutdown => {
                    info!("RPC host: Received shutdown request from main process");
                    break;
                }
            }
        }
        info!("RPC host: Bootstrap confirmation channel closed");
    });

    // Wait for some signal to shutdown
    let ctrl_c = tokio::signal::ctrl_c();

    tokio::select! {
        _ = bootstrap_monitor => {
            info!("RPC host: Bootstrap monitor completed");
        }
        _ = ctrl_c => {
            info!("RPC host: Ctrl-C received, shutting down");
        }
    }

    // Graceful shutdown
    server.shutdown().await;
    bridge_task.await?;

    Ok(())
}
