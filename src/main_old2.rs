mod rpc_ipc_framework2;

use crate::rpc_ipc_framework2::{
    MainProcessBootstrap, MainProcessClientFactory, PluginHostBootstrapClient, RpcClientConfig,
    RpcHostServerFactory, RpcServerConfig,
};
use tarpc::context;
use tracing::Level;

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

        return example_plugin_host(ipc_name).await;
    }

    // Run the main process example
    example_main_process().await
}

// Main Process Example
async fn example_main_process() -> Result<(), Box<dyn std::error::Error>> {
    println!("Starting main process...");

    // Create bootstrap server and wait for plugin host to connect
    let (bootstrap, request_tx, response_rx) =
        MainProcessBootstrap::<PidServiceRequest, PidServiceResponse>::new().await?;

    println!(
        "Tell plugin host to connect to bootstrap: {}",
        bootstrap.bootstrap_name()
    );

    // Create a client that talks to the plugin host
    let (client, bridge_task) = MainProcessClientFactory::create_client(
        request_tx,
        response_rx,
        RpcClientConfig::default(),
        PidServiceClient::new,
    )?;

    // Make RPC calls to the plugin host
    let pid = client.inner_client().get_pid(context::current()).await?;
    println!("Plugin host PID: {}", pid);

    // Graceful shutdown
    client.shutdown().await;
    bootstrap.shutdown().await;
    bridge_task.await?;

    Ok(())
}

// Plugin Host Example
async fn example_plugin_host(bootstrap_name: &str) -> Result<(), Box<dyn std::error::Error>> {
    // Connect to the main process
    let (request_rx, response_tx) =
        PluginHostBootstrapClient::connect::<PidServiceRequest, PidServiceResponse>(bootstrap_name)
            .await?;

    // Create a server to handle requests from the main process
    let (server, bridge_task) = RpcHostServerFactory::create_server(
        PidServer,
        |s| s.serve(),
        request_rx,
        response_tx,
        RpcServerConfig::default(),
    )
    .await?;

    // Wait for some signal to shutdown
    tokio::signal::ctrl_c().await?;

    // Graceful shutdown
    server.shutdown().await;
    bridge_task.await?;

    Ok(())
}
