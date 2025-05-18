use futures::{SinkExt, StreamExt};
use ipc_channel::ipc::{self, IpcReceiver, IpcSender};
use serde::{Deserialize, Serialize};
use std::future::Future;
use std::marker::PhantomData;
use tarpc::{
    ClientMessage, Response, Transport,
    client::{self, Config as ClientConfig, NewClient, RpcError},
    server::{self, Channel},
};
use tokio::{
    sync::broadcast,
    task::JoinHandle,
    time::{self, Duration},
};
use tracing::{error, info, warn};

/// Bootstrap message containing the IPC channels for bidirectional communication
#[derive(Debug, Serialize, Deserialize)]
pub struct RpcBootstrap<Req, Resp>
where
    Req: Send + 'static,
    Resp: Send + 'static,
{
    pub request_tx: IpcSender<ClientMessage<Req>>,
    pub response_rx: IpcReceiver<Response<Resp>>,
}

/// Configuration for the RPC client (used by the main process)
pub struct RpcClientConfig {
    /// Timeout for client calls
    pub timeout_ms: u64,
    /// Timeout for graceful shutdown
    pub shutdown_timeout_ms: u64,
}

impl Default for RpcClientConfig {
    fn default() -> Self {
        Self {
            timeout_ms: 5000,
            shutdown_timeout_ms: 5000,
        }
    }
}

/// Configuration for the RPC server (used by the plugin host)
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

/// A client for an RPC service over IPC, used by the main process to communicate with plugin hosts
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
    pub async fn shutdown(mut self) {
        info!("Shutting down RPC client gracefully...");

        // Drop the client to close the channel
        std::mem::drop(self.client);

        // Wait for the dispatch task to complete with timeout
        let dispatch_handle = std::mem::replace(
            &mut self.dispatch_handle,
            tokio::task::spawn(async {
                Ok(()) as Result<(), tarpc::ChannelError<tarpc::transport::channel::ChannelError>>
            }),
        );
        match time::timeout(
            Duration::from_millis(self.config.shutdown_timeout_ms),
            dispatch_handle,
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
                self.dispatch_handle.abort();
            }
        }

        info!("RPC client shutdown complete");
    }
}

/// A server for an RPC service over IPC, used by the plugin host to expose services to the main process
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
    pub fn new<T, ServeObj>(
        service: S,
        service_future: impl FnOnce(S) -> ServeObj + Send,
        transport: T,
        config: RpcServerConfig,
    ) -> Self
    where
        T: Transport<Response<Resp>, ClientMessage<Req>> + Send + 'static,
        ServeObj: server::Serve<Req = Req, Resp = Resp> + Clone + Send + 'static,
    {
        let (shutdown_token, shutdown_listener) = broadcast::channel::<()>(1);
        let mut server_shutdown = shutdown_listener.resubscribe();

        let _stream =
            server::BaseChannel::with_defaults(transport).execute(service_future(service));

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
            _service: PhantomData,
            _req: PhantomData,
            _resp: PhantomData,
        }
    }

    /// Shutdown the server gracefully
    pub async fn shutdown(mut self) {
        info!("Shutting down RPC server gracefully...");

        // Signal the server to shut down
        let _ = self.shutdown_token.send(());

        // Wait for the server task to complete with timeout
        let server_handle =
            std::mem::replace(&mut self.server_handle, tokio::task::spawn(async {}));
        match time::timeout(
            Duration::from_millis(self.config.shutdown_timeout_ms),
            server_handle,
        )
        .await
        {
            Ok(result) => {
                if let Err(e) = result {
                    error!("RPC server task error: {}", e);
                } else {
                    info!("RPC server task completed successfully");
                }
            }
            Err(_) => {
                warn!("RPC server task did not complete within timeout, aborting");
                self.server_handle.abort();
            }
        }

        info!("RPC server shutdown complete");
    }
}

/// Main Process: Bootstrap Server Creator
/// Creates a bootstrap server to coordinate with plugin hosts
pub struct MainProcessBootstrap<Req, Resp>
where
    Req: tarpc::RequestName + Serialize + for<'de> Deserialize<'de> + Send + 'static,
    Resp: Serialize + for<'de> Deserialize<'de> + Send + 'static,
{
    bootstrap_name: String,
    host_task: JoinHandle<()>,
    shutdown_token: broadcast::Sender<()>,
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

        // Extract the request and response channels from the bootstrap
        let RpcBootstrap {
            request_tx,
            response_rx,
        } = bootstrap;

        // Create shutdown coordination
        let (shutdown_token, host_shutdown_listener) = broadcast::channel::<()>(1);

        // Start a task to manage the connection lifecycle
        let host_task = tokio::spawn(async move {
            let mut shutdown_signal = host_shutdown_listener;

            if let Err(_) = shutdown_signal.recv().await {
                info!("Main process host task shutdown signal receiver closed");
            } else {
                info!("Main process host task received shutdown signal");
            }

            info!("Main process host task exiting");
        });

        Ok((
            Self {
                bootstrap_name,
                host_task,
                shutdown_token,
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

        let _ = self.shutdown_token.send(());

        if let Err(e) = self.host_task.await {
            error!("Error shutting down main process host task: {}", e);
        } else {
            info!("Main process host task shutdown successfully");
        }

        info!("Main process bootstrap shutdown complete");
    }
}

/// Main Process: Client Factory
/// Used by the main process to create a client that connects to a plugin host
pub struct MainProcessClientFactory;

impl MainProcessClientFactory {
    /// Create a new client that communicates with a plugin host via IPC channels
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
    ) -> Result<(RpcClient<C, Req, Resp>, JoinHandle<()>), Box<dyn std::error::Error>>
    where
        C: Clone + Send + 'static,
        Req: tarpc::RequestName + Serialize + for<'de> Deserialize<'de> + Send + 'static,
        Resp: Serialize + for<'de> Deserialize<'de> + Send + 'static,
    {
        // Create tarpc transport channels for the client to use
        let (client_transport, server_transport) = tarpc::transport::channel::unbounded();

        // Create a task to bridge between IPC and tarpc
        let bridge_task = tokio::spawn(async move {
            let mut response_rx = ipc_response_rx.to_stream();
            let mut server_transport = server_transport.fuse();

            loop {
                tokio::select! {
                    message = response_rx.next() => {
                        match message {
                            Some(Ok(message)) => {
                                info!("Main process: Received response via IPC");
                                if let Err(err) = server_transport.send(message).await {
                                    error!("Main process: Error sending response to transport: {}", err);
                                }
                            }
                            Some(Err(err)) => {
                                error!("Main process: Error receiving IPC message: {}", err);
                                break;
                            }
                            None => {
                                info!("Main process: IPC response channel closed");
                                break;
                            }
                        }
                    }
                    message = server_transport.next() => {
                        match message {
                            Some(Ok(message)) => {
                                info!("Main process: Received request via transport");
                                if let Err(err) = ipc_request_tx.send(message) {
                                    error!("Main process: Error sending request via IPC: {}", err);
                                }
                            }
                            Some(Err(err)) => {
                                error!("Main process: Error receiving transport message: {}", err);
                                break;
                            }
                            None => {
                                info!("Main process: Transport channel closed");
                                break;
                            }
                        }
                    }
                }
            }

            // Clean up when loop ends
            info!("Main process: Bridge task exiting, closing transport");
            if let Err(e) = server_transport.close().await {
                error!("Main process: Error closing server transport: {}", e);
            }
        });

        // Create the client
        let new_client = create(ClientConfig::default(), client_transport);
        let client = RpcClient::<C, Req, Resp>::new(new_client, config);

        Ok((client, bridge_task))
    }
}

/// Plugin Host: Plugin Host Bootstrap Client
/// Used by plugin host to connect back to the main process
pub struct PluginHostBootstrapClient;

impl PluginHostBootstrapClient {
    /// Connect to a bootstrap server in the main process
    pub async fn connect<Req, Resp>(
        bootstrap_name: &str,
    ) -> Result<
        (IpcReceiver<ClientMessage<Req>>, IpcSender<Response<Resp>>),
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

        // Send bootstrap message with our channels
        bootstrap.send(RpcBootstrap {
            request_tx,
            response_rx,
        })?;

        info!(
            "Plugin host: Connected to main process bootstrap with name: {}",
            bootstrap_name
        );

        Ok((request_rx, response_tx))
    }
}

/// RPC Host: Server Factory
/// Used by RPC host to create a server that exposes services to the main process
pub struct RpcHostServerFactory;

impl RpcHostServerFactory {
    pub async fn create_server<S, Req, Resp, ServeObj>(
        service: S,
        service_future: impl FnOnce(S) -> ServeObj + Send + 'static,
        ipc_request_rx: IpcReceiver<ClientMessage<Req>>,
        ipc_response_tx: IpcSender<Response<Resp>>,
        config: RpcServerConfig,
    ) -> Result<(RpcServer<S, Req, Resp>, JoinHandle<()>), Box<dyn std::error::Error>>
    where
        S: Clone + Send + 'static,
        Req: tarpc::RequestName + Send + Serialize + for<'de> Deserialize<'de> + 'static,
        Resp: Send + Serialize + for<'de> Deserialize<'de> + 'static,
        ServeObj: server::Serve<Req = Req, Resp = Resp> + Clone + Send + 'static,
    {
        // Create tarpc transport channels for the server to use
        let (tx_channel, rx_channel) = tarpc::transport::channel::unbounded();

        // Create a task to bridge between IPC and tarpc
        let bridge_task = tokio::spawn(async move {
            let mut request_rx = ipc_request_rx.to_stream();
            let mut tx_channel = tx_channel;

            loop {
                tokio::select! {
                    message = request_rx.next() => {
                        match message {
                            Some(Ok(message)) => {
                                info!("Plugin host: Received request via IPC");
                                if let Err(err) = tx_channel.send(message).await {
                                    error!("Plugin host: Error sending request to transport: {:?}", err);
                                    break;
                                }
                            }
                            Some(Err(err)) => {
                                error!("Plugin host: Error receiving IPC message: {}", err);
                                break;
                            }
                            None => {
                                info!("Plugin host: IPC request channel closed");
                                break;
                            }
                        }
                    }
                    // Listen for response messages from the channel
                    message = tx_channel.next() => {
                        match message {
                            Some(Ok(resp)) => {
                                info!("Plugin host: Received response via transport");
                                if let Err(err) = ipc_response_tx.send(resp) {
                                    error!("Plugin host: Error sending response via IPC: {}", err);
                                    break;
                                }
                            }
                            Some(Err(err)) => {
                                error!("Plugin host: Error receiving transport message: {:?}", err);
                                break;
                            }
                            None => {
                                info!("Plugin host: Transport channel closed");
                                break;
                            }
                        }
                    }
                }
            }

            // Clean up when loop ends
            info!("Plugin host: Bridge task exiting");
        });

        // Create the server
        let server = RpcServer::new(service, service_future, rx_channel, config);

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

// Example usage for PidService (similar to your existing code)
#[cfg(test)]
mod examples {
    use tarpc::context;

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

    /*
    // Main Process Example
    async fn example_main_process() -> Result<(), Box<dyn std::error::Error>> {
        // Create bootstrap server and wait for plugin host to connect
        let (bootstrap, request_tx, response_rx) = MainProcessBootstrap::<PidServiceRequest, PidServiceResponse>::new().await?;

        println!("Tell plugin host to connect to bootstrap: {}", bootstrap.bootstrap_name());

        // Create a client that talks to the plugin host
        let (client, bridge_task) = MainProcessClientFactory::create_client::<PidServiceClient, _, _>(
            request_tx,
            response_rx,
            RpcClientConfig::default()
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
        let bootstrap_name_str = bootstrap_name.to_string();
        let (request_rx, response_tx) = PluginHostBootstrapClient::connect::<PidServiceRequest, PidServiceResponse>(bootstrap_name_str).await?;

        // Create a server to handle requests from the main process
        let (server, bridge_task) = PluginHostServerFactory::create_server(
            PidServer,
            |s| s.serve(),
            request_rx,
            response_tx,
            RpcServerConfig::default()
        ).await?;

        // Wait for some signal to shutdown
        tokio::signal::ctrl_c().await?;

        // Graceful shutdown
        server.shutdown().await;
        bridge_task.await?;

        Ok(())
    }
    */
}
