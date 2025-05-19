use clap::Parser;
use std::time::Duration;
use tracing::Level;

#[derive(Parser, Debug)]
#[command()]
struct PluginHostConfig {
    #[arg(long, env = "UPSCLR_SERVER_MODE", default_value = "main")]
    mode: String,

    #[arg(long, env = "UPSCLR_SERVER_IPC")]
    ipc: String,
}

async fn main_wrapper() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_max_level(Level::DEBUG)
        .with_target(true)
        .with_file(true)
        .with_line_number(true)
        .init();

    // Check if we are running in plugin host mode
    if let Ok(config) = PluginHostConfig::try_parse() {
        if config.mode == "plugin_host" {
            return upsclr_server::plugin_host::main(&config.ipc).await;
        }
    }

    // Run the main process
    upsclr_server::main_process::main().await
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("Failed to create Tokio runtime")
        .block_on(async {
            let result = main_wrapper().await.inspect_err(|err| {
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
