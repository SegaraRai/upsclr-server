use futures::{Future, Sink, SinkExt, Stream, StreamExt, ready};
use ipc_channel::ipc;
use serde::{Deserialize, Serialize};
use std::io;
use std::marker::PhantomData;
use std::task::{Context, Poll};
use std::{pin::Pin, process};
use tarpc::{ClientMessage, Response, context};

#[derive(Debug, Serialize, Deserialize)]
pub struct Bootstrap {
    pub request_tx: ipc::IpcSender<ClientMessage<PidServiceRequest>>,
    pub response_rx: ipc::IpcReceiver<Response<PidServiceResponse>>,
}

// Define the service with tarpc
#[tarpc::service]
pub trait PidService {
    /// Returns the process ID of the current process
    async fn get_pid() -> u32;
}
