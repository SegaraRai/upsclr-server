use std::time::Duration;

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

        return upsclr_server::plugin_host::main(ipc_name).await;
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
