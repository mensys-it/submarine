//! IPC server: one task per client, requests handled concurrently, service events
//! and log lines forwarded to every client.
//!
//! Each client gets three tasks: a reader (the client task itself) that spawns one
//! task per request, a writer that serializes every outgoing message, and an event
//! forwarder. They all send to the writer through the same channel, so messages
//! are never interleaved on the socket.

use std::io;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use interprocess::local_socket::tokio::prelude::*;
use submarine_ipc::{
    ClientMessage, Connection, Event, Listener, ServerMessage, read_message, write_message,
};
use tokio::io::BufReader;
use tokio::sync::{broadcast, mpsc};

use crate::service::Service;

/// Message for a user who may not use the service (see `access`).
const REFUSED: &str =
    "this user is not allowed to use Submarine on this computer: ask an administrator";
/// How long a refused client may take to send its first request, which is answered
/// with [`REFUSED`].
const REFUSE_TIMEOUT: Duration = Duration::from_secs(5);

/// Accepts clients forever, each one served by its own task. `data_dir` holds the
/// access file, read again for every client.
///
/// A failed `accept` is logged and does not stop the server.
pub async fn serve(listener: Listener, service: Arc<Service>, data_dir: PathBuf) -> io::Result<()> {
    let data_dir = Arc::new(data_dir);
    loop {
        match listener.accept().await {
            Ok(conn) => {
                let (service, data_dir) = (service.clone(), data_dir.clone());
                tokio::spawn(async move {
                    // users not allowed get neither events nor answers
                    match crate::access::check(&conn, &data_dir) {
                        Ok(()) => handle_client(conn, service).await,
                        Err(reason) => {
                            tracing::warn!("client refused: {reason}");
                            refuse(conn).await;
                        }
                    }
                });
            }
            Err(err) => tracing::warn!("accept failed: {err}"),
        }
    }
}

/// Answers the first request of a refused client with [`REFUSED`], so that the user
/// learns why, then closes the connection.
async fn refuse(conn: Connection) {
    let (reader, mut writer) = conn.split();
    let mut reader = BufReader::new(reader);
    let mut buf = String::new();
    let first = tokio::time::timeout(
        REFUSE_TIMEOUT,
        read_message::<ClientMessage, _>(&mut reader, &mut buf),
    )
    .await;
    if let Ok(Ok(Some(ClientMessage { id, .. }))) = first {
        let message = ServerMessage::Response {
            id,
            result: Err(REFUSED.into()),
        };
        let _ = write_message(&mut writer, &message).await;
    }
}

/// Serves a single client until it disconnects or sends an invalid message.
async fn handle_client(conn: Connection, service: Arc<Service>) {
    tracing::debug!("client connected");
    let (reader, mut writer) = conn.split();
    let (tx, mut rx) = mpsc::channel::<ServerMessage>(64);

    // writer: the only task writing to the socket, stops at the first write error
    let writer_task = tokio::spawn(async move {
        while let Some(message) = rx.recv().await {
            if write_message(&mut writer, &message).await.is_err() {
                break;
            }
        }
    });

    // forwarding of service events and new log lines to this client
    let mut events = service.subscribe();
    let mut logs = crate::logbuf::subscribe();
    let event_tx = tx.clone();
    let events_task = tokio::spawn(async move {
        loop {
            let event = tokio::select! {
                event = events.recv() => match event {
                    Ok(event) => event,
                    // a slow client only misses intermediate states: the next
                    // event carries the full current status anyway
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(broadcast::error::RecvError::Closed) => break,
                },
                line = logs.recv() => match line {
                    Ok(line) => Event::LogLine(line),
                    // missed lines are still in the buffer (`GetLogs`)
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(broadcast::error::RecvError::Closed) => break,
                },
            };
            if event_tx.send(ServerMessage::Event { event }).await.is_err() {
                break;
            }
        }
    });

    // reading of requests: each one is handled in its own task, so a slow request
    // (e.g. a connect) does not hold back the others; responses carry the request
    // id because they can arrive out of order
    let mut reader = BufReader::new(reader);
    let mut buf = String::new();
    loop {
        match read_message::<ClientMessage, _>(&mut reader, &mut buf).await {
            Ok(Some(ClientMessage { id, request })) => {
                tracing::debug!(id, ?request, "request");
                let service = service.clone();
                let tx = tx.clone();
                tokio::spawn(async move {
                    let result = service.handle(request).await;
                    let _ = tx.send(ServerMessage::Response { id, result }).await;
                });
            }
            // client closed the connection
            Ok(None) => break,
            Err(err) => {
                tracing::warn!("dropping client after invalid message: {err}");
                break;
            }
        }
    }

    // cleanup: the writer ends once every sender is dropped (pending responses
    // included), after flushing what is still queued
    events_task.abort();
    drop(tx);
    let _ = writer_task.await;
    tracing::debug!("client disconnected");
}
