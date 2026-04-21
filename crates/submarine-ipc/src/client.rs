//! Client side of the IPC protocol, used by the UI.

use std::collections::HashMap;
use std::io;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use interprocess::local_socket::tokio::{RecvHalf, SendHalf, prelude::*};
use tokio::io::BufReader;
use tokio::sync::{Mutex, broadcast, oneshot};

use crate::transport::{connect, read_message, write_message};
use crate::{ClientMessage, Event, Request, Response, ServerMessage};

/// Requests waiting for a response, by id; shared with the read loop.
/// NB: a std mutex is enough since it is never held across an `await`.
type Pending = Arc<std::sync::Mutex<HashMap<u64, oneshot::Sender<Result<Response, String>>>>>;

/// Errors returned by [`Client`].
#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    /// The socket cannot be opened or written to.
    #[error("cannot reach the Submarine service: {0}")]
    Io(#[from] io::Error),
    /// The connection dropped before the response arrived.
    #[error("connection to the Submarine service was lost")]
    Disconnected,
    /// The daemon answered with an error, carried as a message for the user.
    #[error("{0}")]
    Remote(String),
}

/// Connection to the daemon. Requests may be issued concurrently.
pub struct Client {
    /// Write half; the async mutex keeps concurrent messages from interleaving.
    writer: Mutex<SendHalf>,
    pending: Pending,
    /// Id of the next request.
    next_id: AtomicU64,
    /// Template receiver; the only sender lives in the read loop.
    events: broadcast::Receiver<Event>,
}

impl Client {
    /// Connects to the daemon at `path` (see [`crate::socket_path`]) and starts the
    /// background task that dispatches responses and events.
    pub async fn connect(path: &str) -> Result<Self, ClientError> {
        let (reader, writer) = connect(path).await?.split();
        let pending: Pending = Default::default();
        // subscribers that fall more than 64 events behind miss the oldest ones
        let (sender, events) = broadcast::channel(64);
        tokio::spawn(read_loop(reader, pending.clone(), sender));
        Ok(Self {
            writer: Mutex::new(writer),
            pending,
            next_id: AtomicU64::new(1),
            events,
        })
    }

    /// Sends a request and waits for its response.
    ///
    /// # Errors
    /// [`ClientError::Io`] if the request cannot be written, [`ClientError::Disconnected`] if
    /// the connection drops before the response, [`ClientError::Remote`] if the daemon
    /// reports an error.
    pub async fn request(&self, request: Request) -> Result<Response, ClientError> {
        // the response channel is registered before writing, so a fast answer is not lost
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        self.pending.lock().unwrap().insert(id, tx);

        let message = ClientMessage { id, request };
        // on a write failure no response will come: the pending entry is removed
        if let Err(err) = write_message(&mut *self.writer.lock().await, &message).await {
            self.pending.lock().unwrap().remove(&id);
            return Err(err.into());
        }
        match rx.await {
            Ok(result) => result.map_err(ClientError::Remote),
            Err(_) => Err(ClientError::Disconnected),
        }
    }

    /// Events pushed by the daemon. The stream ends when the connection drops.
    pub fn subscribe(&self) -> broadcast::Receiver<Event> {
        self.events.resubscribe()
    }
}

/// Dispatches incoming messages until the connection closes or a malformed message
/// arrives: responses go to the matching pending request, events to the subscribers.
async fn read_loop(reader: RecvHalf, pending: Pending, events: broadcast::Sender<Event>) {
    let mut reader = BufReader::new(reader);
    let mut buf = String::new();
    loop {
        match read_message::<ServerMessage, _>(&mut reader, &mut buf).await {
            Ok(Some(ServerMessage::Response { id, result })) => {
                // responses to requests no longer waiting are discarded
                if let Some(tx) = pending.lock().unwrap().remove(&id) {
                    let _ = tx.send(result);
                }
            }
            Ok(Some(ServerMessage::Event { event })) => {
                // sending fails only when nobody is subscribed, which is fine
                let _ = events.send(event);
            }
            Ok(None) | Err(_) => break,
        }
    }
    // dropping the senders fails every in-flight request with `Disconnected`,
    // and dropping `events` ends the subscribers' streams
    pending.lock().unwrap().clear();
}
