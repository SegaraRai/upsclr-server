//! # Bootstrap module for IPC communication
//!
//! This module facilitates inter-process communication (IPC) bootstrapping between separate processes.
//!
//! ## Overview
//!
//! The bootstrapping process establishes secure communication paths between processes and
//! exchanges bidirectional channels through which data flows. The architecture follows a
//! host-client model:
//!
//! - The **bootstrap host** creates a named IPC server and waits for connections.
//! - The **bootstrap client** connects to this named server and establishes bidirectional
//!   communication channels.
//!
//! ## Data Flow
//!
//! This module is transport-agnostic regarding the format of data exchanged after bootstrapping.
//! It only requires two distinct message types:
//!
//! - Type `Tx`: Messages sent from host to client
//! - Type `Rx`: Messages sent from client to host
//!
//! Throughout the bootstrap module, we consistently use `Tx` to refer to messages sent from
//! the **host**, and `Rx` for messages sent from the **client**. This naming convention helps
//! distinguish between bootstrap host/client roles and RPC host/client roles, preventing ambiguity.
//!
//! ## Architecture Note
//!
//! Bootstrap host/client roles are distinct from RPC host/client roles:
//!
//! - In this plugin architecture, the bootstrap host (main process) typically acts as an RPC client
//! - The bootstrap client (plugin process) typically acts as an RPC host
//!
//! This separation of concerns allows for flexible communication patterns after the initial
//! connection is established.

use super::transport::IpcTransport;
use ipc_channel::ipc::{self, IpcOneShotServer, IpcReceiver, IpcSender};
use serde::{Deserialize, Serialize};
use std::fmt::Debug;
use tokio::time::{Duration, error::Elapsed};

/// Bootstrap message containing the IPC channels for bidirectional communication.
///
/// This message is exchanged during the bootstrapping process to establish a communication
/// channel between processes. It contains:
///
/// - `tx`: Sender channel for transmitting data from one process to another
/// - `rx`: Receiver channel for receiving data from the other process
///
/// ## Type Parameters
///
/// - `Tx`: Type of messages sent by the host (received by the client)
/// - `Rx`: Type of messages received by the host (sent by the client)
#[derive(Debug, Serialize, Deserialize)]
pub struct BootstrapMessage<Tx, Rx>
where
    Tx: Send + 'static,
    Rx: Send + 'static,
{
    tx: IpcSender<Tx>,
    rx: IpcReceiver<Rx>,
}

// =========================================================
// Bootstrap Host
// =========================================================

/// Represents the host side of the bootstrap process.
///
/// The bootstrap host creates a one-shot IPC server and waits for connections from clients.
/// Once a connection is accepted, it establishes bidirectional communication channels.
///
/// ## Type Parameters
///
/// - `Tx`: Type of messages sent by the host (received by the client)
/// - `Rx`: Type of messages received by the host (sent by the client)
///
/// Both types must implement `Serialize`, `Deserialize`, and be `Send + 'static`.
pub struct BootstrapHost<Tx, Rx>
where
    Tx: Serialize + for<'de> Deserialize<'de> + Send + 'static,
    Rx: Serialize + for<'de> Deserialize<'de> + Send + 'static,
{
    server: IpcOneShotServer<BootstrapMessage<Tx, Rx>>,
}

/// Represents a successful bootstrap connection accepted by the host.
///
/// This struct holds the receiver for bootstrap messages and is returned after a successful
/// connection acceptance. The receiver is kept alive to prevent premature channel closure.
///
/// ## Type Parameters
///
/// - `Message`: Type of messages exchanged during bootstrapping
pub struct BootstrapHostAccepted<Message>
where
    Message: Serialize + for<'de> Deserialize<'de> + Send + 'static,
{
    _bootstrap_receiver: IpcReceiver<Message>,
}

impl<Tx, Rx> BootstrapHost<Tx, Rx>
where
    Tx: Serialize + for<'de> Deserialize<'de> + Send + 'static,
    Rx: Serialize + for<'de> Deserialize<'de> + Send + 'static,
{
    /// Creates a new bootstrap host that listens for connections from clients.
    ///
    /// This method initializes a one-shot IPC server and returns both the host instance
    /// and a unique name that clients can use to connect to this server.
    ///
    /// ## Returns
    ///
    /// - `Ok((BootstrapHost, String))`: The created host and the connection name
    /// - `Err(std::io::Error)`: If server creation fails
    ///
    /// ## Example
    ///
    /// ```no_run
    /// # use std::error::Error;
    /// # async fn example() -> Result<(), Box<dyn Error>> {
    /// # use tokio::time::Duration;
    /// let (host, name) = BootstrapHost::<MyTxType, MyRxType>::new()?;
    /// // Share 'name' with client process
    /// let (transport, _) = host.accept(Duration::from_secs(30)).await?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn new() -> Result<(Self, String), std::io::Error> {
        let (server, name) = IpcOneShotServer::new()?;
        tracing::debug!("Bootstrap host created with name: {}", name);

        Ok((Self { server }, name))
    }

    /// Accepts a connection from a client and establishes bidirectional communication.
    ///
    /// This asynchronous method waits for a connection within the specified timeout period.
    /// When a connection is accepted, it creates an `IpcTransport` that can be used for
    /// bidirectional communication with the client.
    ///
    /// **Caution**: The spawned task will block indefinitely if no client connects, resulting in
    /// hanging the process. To prevent this, consider creating a timer to automatically exit the
    /// process if it is no longer needed. See https://github.com/servo/ipc-channel/issues/307.
    ///
    /// ## Arguments
    ///
    /// - `timeout`: Maximum duration to wait for a connection
    ///
    /// ## Returns
    ///
    /// - `Ok((IpcTransport<Tx, Rx>, BootstrapHostAccepted<...>))`: The transport for communication
    ///   and a struct holding the bootstrap receiver to keep it alive
    /// - `Err(BootstrapAcceptError)`: If connection acceptance fails or times out
    ///
    /// ## Example
    ///
    /// ```no_run
    /// # use tokio::time::Duration;
    /// # async fn example(host: BootstrapHost<u32, String>) -> Result<(), BootstrapAcceptError> {
    /// let (transport, _accepted) = host.accept(Duration::from_secs(30)).await?;
    /// // Use transport for communication
    /// # Ok(())
    /// # }
    /// ```
    pub async fn accept(
        self,
        timeout: Duration,
    ) -> Result<
        (
            IpcTransport<Tx, Rx>,
            BootstrapHostAccepted<BootstrapMessage<Tx, Rx>>,
        ),
        BootstrapAcceptError,
    > {
        tracing::debug!(
            "Bootstrap host waiting for connection with timeout: {:?}",
            timeout
        );

        let (bootstrap_receiver, bootstrap) = tokio::time::timeout(
            timeout,
            tokio::task::spawn_blocking(move || {
                tracing::debug!("Blocking task: Waiting for bootstrap server to accept connection");
                let result = self.server.accept();
                match &result {
                    Ok(_) => tracing::debug!("Bootstrap connection accepted successfully"),
                    Err(e) => tracing::error!("Bootstrap connection acceptance failed: {}", e),
                }
                result
            }),
        )
        .await
        .map_err(|elapsed| {
            tracing::error!(
                "Bootstrap host timed out waiting for connection: {}",
                elapsed
            );
            BootstrapAcceptError(BootstrapAcceptErrorKind::Timeout(elapsed))
        })?
        .map_err(|err| {
            tracing::error!("Failed to accept bootstrap connection: {}", err);
            BootstrapAcceptError(BootstrapAcceptErrorKind::JoinError(err))
        })?
        .map_err(|err| {
            tracing::error!("Failed to deserialize bootstrap message: {}", err);
            BootstrapAcceptError(BootstrapAcceptErrorKind::Serialization(err))
        })?;

        tracing::debug!("Bootstrap host received channels, creating IPC transport");

        let transport = IpcTransport::new(bootstrap.tx, bootstrap.rx);

        Ok((
            transport,
            BootstrapHostAccepted {
                _bootstrap_receiver: bootstrap_receiver,
            },
        ))
    }
}

/// Error type representing failures during bootstrap connection acceptance.
///
/// This error wraps a `BootstrapAcceptErrorKind` and provides detailed error messages
/// and source error information through the `std::error::Error` trait.
#[derive(Debug)]
pub struct BootstrapAcceptError(BootstrapAcceptErrorKind);

/// Represents the different types of errors that can occur when accepting a bootstrap connection.
#[derive(Debug)]
#[non_exhaustive]
pub enum BootstrapAcceptErrorKind {
    /// Connection acceptance timed out
    Timeout(Elapsed),
    /// Failed to deserialize the bootstrap message
    Serialization(ipc_channel::Error),
    /// Error joining the asynchronous task
    JoinError(tokio::task::JoinError),
}

impl std::fmt::Display for BootstrapAcceptError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.0 {
            BootstrapAcceptErrorKind::Timeout(elapsed) => {
                write!(f, "bootstrap host accept timed out after {:?}", elapsed)
            }
            BootstrapAcceptErrorKind::Serialization(err) => {
                write!(f, "failed to deserialize bootstrap message: {}", err)
            }
            BootstrapAcceptErrorKind::JoinError(err) => {
                write!(f, "failed to join bootstrap accept task: {}", err)
            }
        }
    }
}

impl std::error::Error for BootstrapAcceptError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match &self.0 {
            BootstrapAcceptErrorKind::Timeout(_) => None,
            BootstrapAcceptErrorKind::Serialization(err) => Some(err),
            BootstrapAcceptErrorKind::JoinError(err) => Some(err),
        }
    }
}

// =========================================================
// Bootstrap Client
// =========================================================

/// Represents the client side of the bootstrap process.
///
/// The bootstrap client connects to a named bootstrap server and establishes bidirectional
/// communication channels. Once connected, it can exchange messages with the host.
///
/// ## Type Parameters
///
/// - `Tx`: Type of messages sent by the **host** (received by the client)
/// - `Rx`: Type of messages received by the **host** (sent by the client)
///
/// Both types must implement `Serialize`, `Deserialize`, and be `Send + 'static`.
pub struct BootstrapClient<Tx, Rx>
where
    Tx: Serialize + for<'de> Deserialize<'de> + Send + 'static,
    Rx: Serialize + for<'de> Deserialize<'de> + Send + 'static,
{
    _bootstrap_sender: IpcSender<BootstrapMessage<Tx, Rx>>,
}

impl<Tx, Rx> BootstrapClient<Tx, Rx>
where
    Tx: Serialize + for<'de> Deserialize<'de> + Send + 'static,
    Rx: Serialize + for<'de> Deserialize<'de> + Send + 'static,
{
    /// Connects to a bootstrap host and establishes bidirectional communication.
    ///
    /// This method connects to the named bootstrap server and creates IPC channels for
    /// bidirectional communication with the host. It returns an `IpcTransport` that can
    /// be used to send and receive messages.
    ///
    /// ## Arguments
    ///
    /// - `bootstrap_name`: The unique name of the bootstrap server to connect to
    ///
    /// ## Returns
    ///
    /// - `Ok((IpcTransport<Rx, Tx>, BootstrapClient<Tx, Rx>))`: The transport for communication
    ///   and the client instance
    /// - `Err(BootstrapConnectError)`: If connection or channel creation fails
    ///
    /// ## Example
    ///
    /// ```no_run
    /// # fn example(bootstrap_name: &str) -> Result<(), BootstrapConnectError> {
    /// let (transport, _client) = BootstrapClient::<MyTxType, MyRxType>::connect(bootstrap_name)?;
    /// // Use transport for communication
    /// # Ok(())
    /// # }
    /// ```
    pub fn connect(
        bootstrap_name: &str,
    ) -> Result<(IpcTransport<Rx, Tx>, Self), BootstrapConnectError> {
        tracing::debug!("Bootstrap client connecting to: {}", bootstrap_name);

        let bootstrap_sender = IpcSender::<BootstrapMessage<Tx, Rx>>::connect(
            bootstrap_name.to_string(),
        )
        .map_err(|err| {
            tracing::error!("Failed to connect to bootstrap IPC channel: {}", err);
            BootstrapConnectError(BootstrapConnectErrorKind::Connection(err))
        })?;

        tracing::debug!("Connected to bootstrap IPC channel: {}", bootstrap_name);

        // Create transport channels for bidirectional communication
        tracing::debug!("Creating transport channels for bidirectional communication");

        let (host_tx, client_rx) = ipc::channel::<Tx>().map_err(|err| {
            tracing::error!("Failed to create Host TX -> Client RX channel: {}", err);
            BootstrapConnectError(BootstrapConnectErrorKind::ChannelCreation(err))
        })?;

        tracing::debug!("Host TX -> Client RX channel created successfully");

        let (client_tx, host_rx) = ipc::channel::<Rx>().map_err(|err| {
            tracing::error!("Failed to create Client TX -> Host RX channel: {}", err);
            BootstrapConnectError(BootstrapConnectErrorKind::ChannelCreation(err))
        })?;

        tracing::debug!("Client TX -> Host RX channel created successfully");

        let bootstrap_message = BootstrapMessage {
            tx: host_tx,
            rx: host_rx,
        };

        bootstrap_sender.send(bootstrap_message).map_err(|err| {
            tracing::error!("Failed to send transport channels over IPC: {}", err);
            BootstrapConnectError(BootstrapConnectErrorKind::SendError(err))
        })?;

        tracing::debug!("Transport channels sent successfully");

        let transport = IpcTransport::new(client_tx, client_rx);

        Ok((
            transport,
            Self {
                _bootstrap_sender: bootstrap_sender,
            },
        ))
    }
}

/// Error type representing failures during bootstrap client connection.
///
/// This error wraps a `BootstrapConnectErrorKind` and provides detailed error messages
/// and source error information through the `std::error::Error` trait.
#[derive(Debug)]
pub struct BootstrapConnectError(BootstrapConnectErrorKind);

/// Represents the different types of errors that can occur when connecting as a bootstrap client.
#[derive(Debug)]
#[non_exhaustive]
pub enum BootstrapConnectErrorKind {
    ///Failed to connect to the bootstrap server
    Connection(std::io::Error),
    /// Failed to create the IPC channels for communication
    ChannelCreation(std::io::Error),
    /// Failed to send the bootstrap message over IPC
    SendError(ipc_channel::Error),
}

impl std::fmt::Display for BootstrapConnectError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.0 {
            BootstrapConnectErrorKind::Connection(err) => {
                write!(f, "failed to connect to bootstrap IPC channel: {}", err)
            }
            BootstrapConnectErrorKind::ChannelCreation(err) => {
                write!(f, "failed to create transport channels: {}", err)
            }
            BootstrapConnectErrorKind::SendError(err) => {
                write!(f, "failed to send transport channels over IPC: {}", err)
            }
        }
    }
}

impl std::error::Error for BootstrapConnectError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match &self.0 {
            BootstrapConnectErrorKind::Connection(err) => Some(err),
            BootstrapConnectErrorKind::ChannelCreation(err) => Some(err),
            BootstrapConnectErrorKind::SendError(err) => Some(err),
        }
    }
}
