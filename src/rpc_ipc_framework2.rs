use futures::{SinkExt, StreamExt};
use ipc_channel::ipc::{self, IpcReceiver, IpcSender};
use serde::{Deserialize, Serialize};
use std::future::Future;
use std::marker::PhantomData;
use tarpc::{
    ClientMessage, Response, Transport,
    client::{Config as ClientConfig, NewClient, RpcError},
    server,
};
use tokio::time::timeout;
use tokio::{
    sync::broadcast,
    task::JoinHandle,
    time::{self, Duration},
};
use tracing::{error, info, warn};

/// Simple confirmation message to ensure bootstrap process completes
#[derive(Debug, Serialize, Deserialize, Clone)]
pub enum BootstrapConfirmation {
    Ready,
    Shutdown,
}

/// Bootstrap message containing the IPC channels for bidirectional communication
#[derive(Debug, Serialize, Deserialize)]
pub struct RpcBootstrap<Req, Resp>
where
    Req: Send + 'static,
    Resp: Send + 'static,
{
    pub request_tx: IpcSender<ClientMessage<Req>>,
    pub response_rx: IpcReceiver<Response<Resp>>,
    pub confirmation_tx: IpcSender<BootstrapConfirmation>,
}

/// Context holder to keep bootstrap connection alive in the RPC host
pub struct BootstrapConnectorContext<Req, Resp>
where
    Req: Send + 'static,
    Resp: Send + 'static,
{
    // Keep the bootstrap sender alive to prevent channel closure
    _bootstrap_sender: IpcSender<RpcBootstrap<Req, Resp>>,
    // Add a confirmation receiver to coordinate with main process
    pub confirmation_rx: IpcReceiver<BootstrapConfirmation>,
}

/// Configuration for the RPC client (used by the main process)
pub struct RpcClientConfig {
    /// Timeout for graceful shutdown
    pub shutdown_timeout_ms: u64,
}

impl Default for RpcClientConfig {
    fn default() -> Self {
        Self {
            shutdown_timeout_ms: 5000,
        }
    }
}

/// Configuration for the RPC server (used by the RPC host)
pub struct RpcServerConfig {
    /// Timeout for graceful shutdown
    pub shutdown_timeout_ms: u64,
}

impl Default for RpcServerConfig {
    fn default() -> Self {
        Self {
            shutdown_timeout_ms: 5000,
        }
    }
}

/// A client for an RPC service over IPC, used by the main process to communicate with RPC hosts
pub struct RpcClient<C, Req, Resp>
where
    Req: tarpc::RequestName,
    Resp: Send + 'static,
    C: Clone + Send + 'static,
{
    /// The inner tarpc client
    client: C,
    /// Handle to the dispatch task
    dispatch_handle:
        JoinHandle<Result<(), tarpc::ChannelError<tarpc::transport::channel::ChannelError>>>,
    /// Configuration
    config: RpcClientConfig,
    /// Phantom data for type parameters
    _req: PhantomData<Req>,
    _resp: PhantomData<Resp>,
}

impl<C, Req, Resp> RpcClient<C, Req, Resp>
where
    Req: tarpc::RequestName,
    Resp: Send + 'static,
    C: Clone + Send + 'static,
{
    /// Create a new RPC client with the given client and transport
    pub fn new(
        new_client: NewClient<
            C,
            impl Future<
                Output = Result<(), tarpc::ChannelError<tarpc::transport::channel::ChannelError>>,
            > + Send
            + 'static,
        >,
        config: RpcClientConfig,
    ) -> Self {
        let client = new_client.client;
        let dispatch = new_client.dispatch;
        let dispatch_handle = tokio::spawn(async move {
            if let Err(e) = dispatch.await {
                error!("RPC client dispatch error: {}", e);
                Err(e)
            } else {
                Ok(())
            }
        });

        Self {
            client,
            dispatch_handle,
            config,
            _req: PhantomData,
            _resp: PhantomData,
        }
    }

    /// Get the inner tarpc client
    pub fn inner_client(&self) -> &C {
        &self.client
    }

    /// Shutdown the client gracefully
    pub async fn shutdown(self) {
        info!("Shutting down RPC client gracefully...");

        // Drop the client to close the channel
        std::mem::drop(self.client);

        match timeout(
            Duration::from_millis(self.config.shutdown_timeout_ms),
            self.dispatch_handle,
        )
        .await
        {
            Ok(result) => match result {
                Ok(inner_result) => match inner_result {
                    Ok(_) => info!("RPC client dispatch task completed successfully"),
                    Err(e) => error!("RPC client dispatch task failed: {}", e),
                },
                Err(e) => error!("RPC client dispatch join error: {}", e),
            },
            Err(_) => {
                warn!("RPC client dispatch task did not complete within timeout, aborting");
            }
        }

        info!("RPC client shutdown complete");
    }
}

/// A server for an RPC service over IPC, used by the RPC host to expose services to the main process
pub struct RpcServer<S, Req, Resp>
where
    S: Send + Clone + 'static,
    Req: Send + 'static,
    Resp: Send + 'static,
{
    /// The shutdown token sender
    shutdown_token: broadcast::Sender<()>,
    /// Handle to the server task
    server_handle: JoinHandle<()>,
    /// Configuration
    config: RpcServerConfig,
    /// The tarpc channel executor which runs the service
    server_executor_task: tokio::task::JoinHandle<()>,
    /// Phantom data for type parameters
    _service: PhantomData<S>,
    _req: PhantomData<Req>,
    _resp: PhantomData<Resp>,
}

impl<S, Req, Resp> RpcServer<S, Req, Resp>
where
    S: Send + Clone + 'static,
    Req: tarpc::RequestName + Send + 'static,
    Resp: Send + 'static,
{
    /// Create a new RPC server with the given service implementation and transport
    pub fn new<T, Fut>(
        service_server: S,
        execute: impl FnOnce(S, server::BaseChannel<Req, Resp, T>) -> Fut,
        transport: T,
        config: RpcServerConfig,
    ) -> Self
    where
        T: Transport<Response<Resp>, ClientMessage<Req>> + Send + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    {
        let (shutdown_token, shutdown_listener) = broadcast::channel::<()>(1);
        let mut server_shutdown = shutdown_listener.resubscribe();

        // Create and store the server executor in an Arc to keep it alive
        let server_executor = execute(
            service_server,
            server::BaseChannel::with_defaults(transport),
        );

        // Wrap the server executor in a task to manage its lifetime
        let server_executor_task = tokio::spawn(async move {
            // This task exists just to keep the executor alive as long as the server is alive
            server_executor.await;
            info!("RPC server executor completed");
        });

        // Wrap the server future with cancellation token
        let server_handle = tokio::spawn(async move {
            tokio::select! {
                // TODO: check for server ends
                _ = server_shutdown.recv() => {
                    info!("RPC server shutdown requested, stopping gracefully");
                }
            }
        });

        Self {
            shutdown_token,
            server_handle,
            config,
            server_executor_task,
            _service: PhantomData,
            _req: PhantomData,
            _resp: PhantomData,
        }
    }

    /// Shutdown the server gracefully
    pub async fn shutdown(self) {
        info!("Shutting down RPC server gracefully...");

        // First clean up the server executor without waiting
        // This helps prevent the "Requests stream errored out" warning
        // by making sure the executor is shut down cleanly before
        // we fully terminate the server task
        self.server_executor_task.abort();

        // Signal the server to shut down
        let _ = self.shutdown_token.send(());

        // Wait for the server task to complete with timeout
        match time::timeout(
            Duration::from_millis(self.config.shutdown_timeout_ms),
            self.server_handle,
        )
        .await
        {
            Ok(result) => match result {
                Ok(()) => info!("RPC server task completed successfully"),
                Err(e) => error!("RPC server task join error: {}", e),
            },
            Err(_) => {
                warn!("RPC server task did not complete within timeout, aborting");
            }
        }

        info!("RPC server shutdown complete");
    }
}

/// Main Process: Bootstrap Server Creator
/// Creates a bootstrap server to coordinate with RPC hosts
pub struct MainProcessBootstrap<Req, Resp>
where
    Req: tarpc::RequestName + Serialize + for<'de> Deserialize<'de> + Send + 'static,
    Resp: Serialize + for<'de> Deserialize<'de> + Send + 'static,
{
    bootstrap_name: String,
    host_task: JoinHandle<()>,
    shutdown_token: broadcast::Sender<()>,
    confirmation_tx: IpcSender<BootstrapConfirmation>,
    _req: PhantomData<Req>,
    _resp: PhantomData<Resp>,
}

impl<Req, Resp> MainProcessBootstrap<Req, Resp>
where
    Req: tarpc::RequestName + Serialize + for<'de> Deserialize<'de> + Send + 'static,
    Resp: Serialize + for<'de> Deserialize<'de> + Send + 'static,
{
    /// Create a new bootstrap server and wait for a connection from a RPC host
    pub async fn new() -> Result<
        (
            Self,
            IpcSender<ClientMessage<Req>>,
            IpcReceiver<Response<Resp>>,
        ),
        Box<dyn std::error::Error>,
    > {
        // Create the bootstrap server
        let (bootstrap_server, bootstrap_name) =
            ipc::IpcOneShotServer::<RpcBootstrap<Req, Resp>>::new()?;

        info!("Bootstrap server created with name: {}", bootstrap_name);

        // Wait for a connection from RPC host
        let (_, bootstrap) = tokio::task::spawn_blocking(|| bootstrap_server.accept()).await??;

        info!("Bootstrap connection accepted from RPC host");

        // Extract the channels from the bootstrap
        let RpcBootstrap {
            request_tx,
            response_rx,
            confirmation_tx,
        } = bootstrap;

        // Send confirmation to RPC host
        confirmation_tx.send(BootstrapConfirmation::Ready)?;

        // Create shutdown coordination
        let (shutdown_token, host_shutdown_listener) = broadcast::channel::<()>(1);

        // Store confirmation sender for shutdown
        let confirmation_tx_clone = confirmation_tx.clone();

        // Start a task to manage the connection lifecycle
        let host_task = tokio::spawn(async move {
            let mut shutdown_signal = host_shutdown_listener;

            if let Err(_) = shutdown_signal.recv().await {
                info!("Main process host task shutdown signal receiver closed");
            } else {
                info!("Main process host task received shutdown signal");
                // Notify RPC host about shutdown
                if let Err(e) = confirmation_tx.send(BootstrapConfirmation::Shutdown) {
                    warn!("Failed to send shutdown confirmation to RPC host: {}", e);
                }
            }

            info!("Main process host task exiting");
        });

        Ok((
            Self {
                bootstrap_name,
                host_task,
                shutdown_token,
                confirmation_tx: confirmation_tx_clone,
                _req: PhantomData,
                _resp: PhantomData,
            },
            request_tx,
            response_rx,
        ))
    }

    /// Get the bootstrap name
    pub fn bootstrap_name(&self) -> &str {
        &self.bootstrap_name
    }

    /// Shutdown the bootstrap host task
    pub async fn shutdown(self) {
        info!("Shutting down main process bootstrap...");

        // Notify RPC host about shutdown
        if let Err(e) = self.confirmation_tx.send(BootstrapConfirmation::Shutdown) {
            warn!("Failed to send shutdown confirmation to RPC host: {}", e);
        }

        let _ = self.shutdown_token.send(());

        if let Err(e) = self.host_task.await {
            error!("Error shutting down main process host task: {}", e);
        } else {
            info!("Main process host task shutdown successfully");
        }

        info!("Main process bootstrap shutdown complete");
    }
}

/// Factory for creating bridge tasks
pub struct BridgeFactory;

pub struct BridgeResult<T>(std::pin::Pin<std::boxed::Box<T>>);

impl BridgeFactory {
    /// Create a bridge between IPC and tarpc channels
    pub fn create_ipc_bridge<Rx, Tx, T>(
        ipc_rx: IpcReceiver<Rx>,
        ipc_tx: IpcSender<Tx>,
        tarpc_transport: T,
    ) -> JoinHandle<BridgeResult<T>>
    where
        Rx: Serialize + for<'de> Deserialize<'de> + Send + 'static,
        Tx: Serialize + for<'de> Deserialize<'de> + Send + 'static,
        T: Transport<Rx, Tx> + Send + 'static,
    {
        tokio::spawn(async move {
            let mut ipc_rx_stream = ipc_rx.to_stream();
            let tarpc_transport = Box::pin(tarpc_transport);
            let mut tarpc_transport = tarpc_transport.fuse();

            loop {
                tokio::select! {
                    // Handle IPC to tarpc direction
                    message = ipc_rx_stream.next() => {
                        match message {
                            Some(Ok(msg)) => {
                                if let Err(err) = tarpc_transport.send(msg).await {
                                    // During shutdown, this is expected and normal
                                    info!("Bridge: Could not send to tarpc transport (likely during shutdown): {}", err);
                                }
                            },
                            Some(Err(err)) => {
                                error!("Bridge: Error receiving from IPC: {}", err);
                            },
                            None => {
                                info!("Bridge: IPC channel closed");
                                break;
                            }
                        }
                    },

                    // Handle tarpc to IPC direction
                    message = tarpc_transport.next() => {
                        match message {
                            Some(Ok(msg)) => {
                                if let Err(err) = ipc_tx.send(msg) {
                                    // During shutdown, this is expected and normal
                                    info!("Bridge: Could not send to IPC (likely during shutdown): {}", err);
                                }
                            },
                            Some(Err(err)) => {
                                error!("Bridge: Error receiving from tarpc transport: {}", err);
                            },
                            None => {
                                info!("Bridge: tarpc transport closed");
                                    break;
                            }
                        }
                    },
                }
            }

            info!("Bridge task exiting");

            BridgeResult(tarpc_transport.into_inner())
        })
    }
}

/// Main Process: Client Factory
/// Used by the main process to create a client that connects to an RPC host
pub struct MainProcessClientFactory;

impl MainProcessClientFactory {
    /// Create a new client that communicates with an RPC host via IPC channels
    pub fn create_client<C, Req, Resp>(
        ipc_request_tx: IpcSender<ClientMessage<Req>>,
        ipc_response_rx: IpcReceiver<Response<Resp>>,
        config: RpcClientConfig,
        create: impl FnOnce(
            tarpc::client::Config,
            tarpc::transport::channel::UnboundedChannel<
                tarpc::Response<Resp>,
                tarpc::ClientMessage<Req>,
            >,
        ) -> tarpc::client::NewClient<
            C,
            tarpc::client::RequestDispatch<
                Req,
                Resp,
                tarpc::transport::channel::UnboundedChannel<
                    tarpc::Response<Resp>,
                    tarpc::ClientMessage<Req>,
                >,
            >,
        >,
    ) -> Result<
        (
            RpcClient<C, Req, Resp>,
            JoinHandle<
                BridgeResult<
                    tarpc::transport::channel::UnboundedChannel<
                        tarpc::ClientMessage<Req>,
                        tarpc::Response<Resp>,
                    >,
                >,
            >,
        ),
        Box<dyn std::error::Error>,
    >
    where
        C: Clone + Send + 'static,
        Req: tarpc::RequestName + Serialize + for<'de> Deserialize<'de> + Send + 'static,
        Resp: Serialize + for<'de> Deserialize<'de> + Send + 'static,
    {
        // Create tarpc transport channels for the client to use
        let (client_transport, server_transport) = tarpc::transport::channel::unbounded();

        // Create a bridge task between IPC and tarpc
        let bridge_task =
            BridgeFactory::create_ipc_bridge(ipc_response_rx, ipc_request_tx, server_transport);

        // Create the client
        let new_client = create(ClientConfig::default(), client_transport);
        let client = RpcClient::<C, Req, Resp>::new(new_client, config);

        Ok((client, bridge_task))
    }
}

/// RPC Host: RPC Host Bootstrap Client
/// Used by RPC host to connect back to the main process
pub struct RpcHostBootstrapClient;

impl RpcHostBootstrapClient {
    /// Connect to a bootstrap server in the main process
    pub async fn connect<Req, Resp>(
        bootstrap_name: &str,
    ) -> Result<
        (
            IpcReceiver<ClientMessage<Req>>,
            IpcSender<Response<Resp>>,
            BootstrapConnectorContext<Req, Resp>,
        ),
        Box<dyn std::error::Error>,
    >
    where
        Req: Serialize + for<'de> Deserialize<'de> + Send + 'static,
        Resp: Serialize + for<'de> Deserialize<'de> + Send + 'static,
    {
        // Connect to the bootstrap server
        let bootstrap = IpcSender::<RpcBootstrap<Req, Resp>>::connect(bootstrap_name.to_string())?;

        // Create IPC channels for bidirectional communication
        let (request_tx, request_rx) = ipc::channel::<ClientMessage<Req>>()?;
        let (response_tx, response_rx) = ipc::channel::<Response<Resp>>()?;

        // Create confirmation channel
        let (confirm_tx, confirm_rx) = ipc::channel::<BootstrapConfirmation>()?;

        // Send bootstrap message with our channels
        bootstrap.send(RpcBootstrap {
            request_tx,
            response_rx,
            confirmation_tx: confirm_tx,
        })?;

        // Store bootstrap sender to prevent it from being dropped
        let context = BootstrapConnectorContext {
            _bootstrap_sender: bootstrap,
            confirmation_rx: confirm_rx,
        };

        info!(
            "RPC host: Connected to main process bootstrap with name: {}",
            bootstrap_name
        );

        Ok((request_rx, response_tx, context))
    }
}

/// RPC Host: Server Factory
/// Used by RPC host to create a server that exposes services to the main process
pub struct RpcHostServerFactory;

impl RpcHostServerFactory {
    pub async fn create_server<S, Req, Resp, Fut>(
        service_server: S,
        execute: impl FnOnce(
            S,
            server::BaseChannel<
                Req,
                Resp,
                tarpc::transport::channel::UnboundedChannel<
                    tarpc::ClientMessage<Req>,
                    tarpc::Response<Resp>,
                >,
            >,
        ) -> Fut,
        ipc_request_rx: IpcReceiver<ClientMessage<Req>>,
        ipc_response_tx: IpcSender<Response<Resp>>,
        config: RpcServerConfig,
    ) -> Result<
        (
            RpcServer<S, Req, Resp>,
            JoinHandle<
                BridgeResult<
                    tarpc::transport::channel::UnboundedChannel<
                        tarpc::Response<Resp>,
                        tarpc::ClientMessage<Req>,
                    >,
                >,
            >,
        ),
        Box<dyn std::error::Error>,
    >
    where
        S: Clone + Send + 'static,
        Req: tarpc::RequestName + Send + Serialize + for<'de> Deserialize<'de> + 'static,
        Resp: Send + Serialize + for<'de> Deserialize<'de> + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    {
        // Create tarpc transport channels for the server to use
        let (tx_channel, rx_channel) = tarpc::transport::channel::unbounded();

        // Create a bridge task between IPC and tarpc
        let bridge_task =
            BridgeFactory::create_ipc_bridge(ipc_request_rx, ipc_response_tx, tx_channel);

        // Create the server
        let server = RpcServer::new(service_server, execute, rx_channel, config);

        Ok((server, bridge_task))
    }
}

/// Extension trait for clients to add timeout to RPC calls
pub trait RpcClientExt<Req, Resp>: Clone
where
    Req: tarpc::RequestName,
    Resp: Send + 'static,
{
    /// Call a remote procedure with a timeout
    async fn call_with_timeout<F, Fut, R>(&self, timeout_ms: u64, f: F) -> Result<R, RpcError>
    where
        F: FnOnce(Self) -> Fut,
        Fut: Future<Output = Result<R, RpcError>>;
}

impl<C, Req, Resp> RpcClientExt<Req, Resp> for C
where
    C: Clone,
    Req: tarpc::RequestName,
    Resp: Send + 'static,
{
    async fn call_with_timeout<F, Fut, R>(&self, timeout_ms: u64, f: F) -> Result<R, RpcError>
    where
        F: FnOnce(Self) -> Fut,
        Fut: Future<Output = Result<R, RpcError>>,
    {
        match time::timeout(Duration::from_millis(timeout_ms), f(self.clone())).await {
            Ok(result) => result,
            Err(_) => Err(RpcError::DeadlineExceeded),
        }
    }
}

// Example usage for PidService
#[cfg(test)]
mod examples {
    use super::*;
    use tarpc::{context, server::Channel};

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
    #[allow(dead_code)]
    async fn example_main_process() -> Result<(), Box<dyn std::error::Error>> {
        // Create bootstrap server and wait for RPC host to connect
        let (bootstrap, request_tx, response_rx) =
            MainProcessBootstrap::<PidServiceRequest, PidServiceResponse>::new().await?;

        println!(
            "Tell RPC host to connect to bootstrap: {}",
            bootstrap.bootstrap_name()
        );

        // Create a client that talks to the RPC host with resilient bridge
        let (client, bridge_task) = MainProcessClientFactory::create_client(
            request_tx,
            response_rx,
            RpcClientConfig::default(),
            PidServiceClient::new,
        )?;

        // Make RPC calls to the RPC host
        let pid = client.inner_client().get_pid(context::current()).await?;
        println!("RPC host PID: {}", pid);

        // Graceful shutdown - first shutdown the client
        client.shutdown().await;

        // Small delay to allow pending operations to complete
        tokio::time::sleep(Duration::from_millis(50)).await;

        // Then shutdown the bootstrap which sends shutdown signal to RPC host
        bootstrap.shutdown().await;

        // Finally wait for bridge task to complete
        bridge_task.await?;

        Ok(())
    }

    // RPC Host Example
    #[allow(dead_code)]
    async fn example_rpc_host(bootstrap_name: &str) -> Result<(), Box<dyn std::error::Error>> {
        // Connect to the main process
        let (request_rx, response_tx, bootstrap_context) = RpcHostBootstrapClient::connect::<
            PidServiceRequest,
            PidServiceResponse,
        >(bootstrap_name)
        .await?;

        // Create a server to handle requests from the main process
        let (server, bridge_task) = RpcHostServerFactory::create_server(
            PidServer,
            |server, channel| {
                channel
                    .execute(server.serve())
                    .for_each(|response| response)
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
                        return; // Exit the monitor loop on shutdown request
                    }
                }
            }
            info!("RPC host: Bootstrap confirmation channel closed");
        });

        // Wait for some signal to shutdown
        tokio::signal::ctrl_c().await?;

        // Graceful shutdown - first cancel bootstrap monitor
        // which might be waiting for messages
        bootstrap_monitor.abort();

        // Small delay to allow pending operations to complete
        tokio::time::sleep(Duration::from_millis(50)).await;

        // Then shutdown the server
        server.shutdown().await;

        // Wait for bridge task to complete
        bridge_task.await?;

        Ok(())
    }
}
