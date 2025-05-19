//! # IPC Transport Module
//!
//! This module provides a bidirectional transport layer over IPC channels for RPC communication.
//! It implements the `Stream` and `Sink` traits from the `futures` crate, making it compatible
//! with async frameworks and the `tarpc` RPC library.
//!
//! The transport layer acts as a bridge between processes, allowing them to exchange serialized
//! messages in a type-safe manner.

use futures::StreamExt;
use ipc_channel::{
    asynch::IpcStream,
    ipc::{IpcError, IpcReceiver, IpcSender},
};
use serde::{Deserialize, Serialize};
use std::fmt::Debug;

/// Transport over IPC channels for RPC communication, compatible with the `tarpc` crate.
///
/// This struct implements both `Stream` and `Sink` traits, allowing for bidirectional
/// communication between processes. It handles serialization and deserialization of
/// messages using the `serde` framework.
///
/// ## Type Parameters
///
/// - `Tx`: Type of messages sent by this transport (must be serializable/deserializable)
/// - `Rx`: Type of messages received by this transport (must be serializable/deserializable)
///
/// ## Usage
///
/// This transport is typically created during the bootstrap process and then used with
/// a tarpc channel to establish RPC communication between processes.
///
/// ```no_run
/// # use tarpc::client;
/// # async fn example<Req, Resp>(transport: IpcTransport<Req, Resp>) {
/// let channel = tarpc::client::new(client::Config::default(), transport);
/// // Use channel for RPC communication
/// # }
/// ```
pub struct IpcTransport<Tx, Rx>
where
    Tx: Serialize + for<'de> Deserialize<'de> + Send + 'static,
    Rx: Serialize + for<'de> Deserialize<'de> + Send + 'static,
{
    tx: IpcSender<Tx>,
    rx: IpcStream<Rx>,
}

impl<Tx, Rx> IpcTransport<Tx, Rx>
where
    Tx: Serialize + for<'de> Deserialize<'de> + Send + 'static,
    Rx: Serialize + for<'de> Deserialize<'de> + Send + 'static,
{
    /// Creates a new IPC transport with the given sender and receiver channels.
    ///
    /// This constructor converts the provided `IpcReceiver` into an `IpcStream`,
    /// which provides the async capabilities needed for the `Stream` trait.
    ///
    /// ## Arguments
    ///
    /// - `tx`: IPC sender for outgoing messages
    /// - `rx`: IPC receiver for incoming messages
    ///
    /// ## Visibility
    ///
    /// This constructor is only visible within the parent module, as transports
    /// should be created through the bootstrap process rather than directly.
    pub(super) fn new(tx: IpcSender<Tx>, rx: IpcReceiver<Rx>) -> Self {
        Self {
            tx,
            rx: rx.to_stream(),
        }
    }
}

/// Implementation of the `Stream` trait for `IpcTransport`.
///
/// This implementation allows the transport to be used as an asynchronous stream of incoming
/// messages. It delegates to the underlying `IpcStream` for polling and error handling.
///
/// The stream yields messages of type `Rx` (the receive type) or errors if deserialization fails.
impl<Tx, Rx> futures::Stream for IpcTransport<Tx, Rx>
where
    Tx: Serialize + for<'de> Deserialize<'de> + Send + 'static + std::marker::Unpin,
    Rx: Serialize + for<'de> Deserialize<'de> + Send + 'static + std::marker::Unpin + Debug,
{
    type Item = Result<Rx, IpcError>;

    /// Attempts to pull out the next value of the stream.
    ///
    /// Returns `Poll::Ready(Some(Ok(value)))` if a value is available, `Poll::Ready(Some(Err(e)))`
    /// if an error occurred, `Poll::Ready(None)` if the stream is finished, or
    /// `Poll::Pending` if no value is ready yet.
    fn poll_next(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        self.get_mut().rx.poll_next_unpin(cx).map_err(|e| {
            tracing::error!("IPC stream error: {}", e);
            IpcError::Bincode(e)
        })
    }
}

/// Implementation of the `Sink` trait for `IpcTransport`.
///
/// This implementation allows the transport to asynchronously send messages to the other process.
/// It uses the underlying `IpcSender` to serialize and transmit messages of type `Tx`.
///
/// The sink is always ready to accept new items since `IpcSender` operations are non-blocking.
impl<Tx, Rx> futures::Sink<Tx> for IpcTransport<Tx, Rx>
where
    Tx: Serialize + for<'de> Deserialize<'de> + Send + 'static + std::marker::Unpin + Debug,
    Rx: Serialize + for<'de> Deserialize<'de> + Send + 'static + std::marker::Unpin,
{
    type Error = IpcError;

    /// Checks if the sink is ready to accept a new item.
    ///
    /// `IpcSender` is always ready to accept new items, so this method always
    /// returns `Poll::Ready(Ok(()))`.
    fn poll_ready(
        self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), Self::Error>> {
        // IpcSender is always ready to send
        std::task::Poll::Ready(Ok(()))
    }

    /// Begins sending a message to the receiver.
    ///
    /// This method serializes the item and immediately sends it over the IPC channel.
    /// It returns an error if serialization or sending fails.
    fn start_send(self: std::pin::Pin<&mut Self>, item: Tx) -> Result<(), Self::Error> {
        // Serialize the item and send it over the IPC channel
        self.get_mut().tx.send(item).map_err(|err| {
            tracing::error!("Failed to send IPC message: {}", err);
            IpcError::Bincode(err)
        })
    }

    /// Flushes any buffered items to ensure they reach the receiver.
    ///
    /// `IpcSender` sends items immediately without buffering, so this method
    /// always returns `Poll::Ready(Ok(()))`.
    fn poll_flush(
        self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), Self::Error>> {
        // IpcSender flushes immediately on send
        std::task::Poll::Ready(Ok(()))
    }

    /// Closes the sink, indicating no more items will be sent.
    ///
    /// `IpcSender` doesn't require an explicit close operation, so this method
    /// always returns `Poll::Ready(Ok(()))`. The channel will be closed when
    /// the `IpcSender` is dropped.
    fn poll_close(
        self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), Self::Error>> {
        // No explicit close needed for IpcSender
        std::task::Poll::Ready(Ok(()))
    }
}
