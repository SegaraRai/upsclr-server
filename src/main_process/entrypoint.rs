use crate::ipc_models::{Bootstrap, PidServiceClient, PidServiceRequest, PidServiceResponse};
use base64::prelude::{BASE64_URL_SAFE_NO_PAD, Engine as _};
use futures::{SinkExt, StreamExt};
use ipc_channel::ipc;
use std::env;
use std::future::Future;
use std::process::{Command, Stdio};
use tarpc::{ClientMessage, Response, client, context};
use tracing::{error, info};

pub async fn main(shutdown_signal: impl Future<Output = ()> + Send + 'static) {
    let current_pid = std::process::id();
    info!("Main process started with PID: {}", current_pid);

    let (client_transport, server_transport) = tarpc::transport::channel::unbounded();

    let (boootstrap_server, boootstrap_name) = ipc::IpcOneShotServer::<Bootstrap>::new().unwrap();
    println!("Bootstrap server created with name: {}", boootstrap_name);

    // Spawn the plugin host process
    /*
    let mut child = match Command::new(env::current_exe().unwrap())
        .arg("--mode=plugin_host")
        .env("UPSCLR_SERVER_IPC", boootstrap_name)
        .stdin(Stdio::null())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn()
    {
        Ok(child) => child,
        Err(e) => {
            error!("Failed to spawn plugin host process: {}", e);
            return;
        }
    };
    */

    println!(
        "Run: `cargo run -- --mode=plugin_host --ipc={}`",
        boootstrap_name
    );

    info!("Plugin host process spawned");

    let (
        _,
        Bootstrap {
            request_tx,
            response_rx,
        },
    ) = boootstrap_server.accept().unwrap();

    let host_task = tokio::spawn(async move {
        let mut response_rx = response_rx.to_stream();
        let mut server_transport = server_transport.fuse();
        let mut shutdown_signal = tokio::spawn(shutdown_signal);

        loop {
            tokio::select! {
                message = response_rx.next() => {
                    match message {
                        Some(Ok(message)) => {
                            info!("Received response via IPC: {:?}", message);
                            let _ = server_transport.send(message).await.inspect_err(|err| {
                                error!("Error sending response: {}", err);
                            });
                        }
                        Some(Err(err)) => {
                            error!("Error receiving IPC message: {}", err);
                        }
                        None => {
                            info!("IPC channel closed");
                            break;
                        }
                    }
                }
                message = server_transport.next() => {
                    match message {
                        Some(Ok(message)) => {
                            info!("Received request via RPC: {:?}", message);
                            let _ = request_tx.send(message).inspect_err(|err| {
                                error!("Error sending IPC message: {}", err);
                            });
                        }
                        Some(Err(err)) => {
                            error!("Error receiving request via RPC: {}", err);
                        }
                        None => {
                            info!("Server transport closed");
                            break;
                        }
                    }
                }
                _ = &mut shutdown_signal => {
                    info!("Shutdown signal received, exiting main process");
                    break;
                }
            }
        }
    });

    // Create a client but do not spawn the dispatch task yet
    let new_client = PidServiceClient::new(client::Config::default(), client_transport);

    // Explicitly extract both client and dispatch
    let client = new_client.client;
    let dispatch = tokio::spawn(new_client.dispatch);

    let hello = client
        .get_pid(context::current())
        .await
        .expect("Failed to call get_pid");
    info!("Received PID from plugin host: {}", hello);

    // Drop the client explicitly to close the channel to the dispatch task
    drop(client);

    // Wait for the dispatch task to complete (should terminate once client is dropped)
    if let Err(e) = dispatch.await {
        error!("Error waiting for client dispatch task to complete: {}", e);
    } else {
        info!("Client dispatch task completed gracefully");
    }

    // Now wait for the host task
    host_task.await.expect("Host task failed");

    info!("Main process shutting down");
}
