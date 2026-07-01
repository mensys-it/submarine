//! Keeps the most recent log lines in memory, so the UI can show them through IPC
//! (on Windows the log file is readable only by administrators), and publishes
//! each new line to the connected clients.
//!
//! It is a `tracing` layer (`BufferLayer`) registered next to the regular output
//! in `init_logging`; the buffer and the channel are process-wide statics.

use std::collections::VecDeque;
use std::fmt::Write;
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use submarine_ipc::LogLine;
use tokio::sync::broadcast;
use tracing::field::{Field, Visit};
use tracing::{Event, Subscriber};
use tracing_subscriber::Layer;
use tracing_subscriber::layer::Context;

/// Maximum number of lines kept in memory; the oldest are dropped first.
const CAPACITY: usize = 1000;
/// Lines a slow client can fall behind before missing some.
const LIVE_BACKLOG: usize = 256;

/// Most recent log lines, oldest first.
static LINES: OnceLock<Mutex<VecDeque<LogLine>>> = OnceLock::new();
/// Channel publishing every new line to the connected clients.
static LIVE: OnceLock<broadcast::Sender<LogLine>> = OnceLock::new();

/// Buffer of the recent lines, created on first use.
fn lines() -> &'static Mutex<VecDeque<LogLine>> {
    LINES.get_or_init(|| Mutex::new(VecDeque::with_capacity(CAPACITY)))
}

/// Live line channel, created on first use.
fn live() -> &'static broadcast::Sender<LogLine> {
    LIVE.get_or_init(|| broadcast::channel(LIVE_BACKLOG).0)
}

/// Subscribes to the lines logged from now on.
pub fn subscribe() -> broadcast::Receiver<LogLine> {
    live().subscribe()
}

/// Returns a copy of the buffered lines, oldest first.
pub fn snapshot() -> Vec<LogLine> {
    lines().lock().unwrap().iter().cloned().collect()
}

/// Empties the buffer. Clients already subscribed are not affected.
pub fn clear() {
    lines().lock().unwrap().clear();
}

/// `tracing` layer that stores each event in the buffer and publishes it live.
pub struct BufferLayer;

impl<S: Subscriber> Layer<S> for BufferLayer {
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        // formatting of the message and its fields
        let mut visitor = Message::default();
        event.record(&mut visitor);
        // timestamp in milliseconds since the Unix epoch (0 if the clock is broken)
        let time = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or_default();
        let line = LogLine {
            time,
            level: event.metadata().level().to_string().to_lowercase(),
            target: event.metadata().target().to_owned(),
            message: visitor.0,
        };
        // storage, dropping the oldest line when full; the lock is released
        // before publishing
        {
            let mut lines = lines().lock().unwrap();
            if lines.len() == CAPACITY {
                lines.pop_front();
            }
            lines.push_back(line.clone());
        }
        // live publishing: it never blocks and never logs, so it cannot recurse
        // into this layer (an error only means no client is listening)
        let _ = live().send(line);
    }
}

/// Field visitor that formats an event as its `message` first, then the other
/// fields as `key=value`, separated by spaces.
#[derive(Default)]
struct Message(String);

impl Visit for Message {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        // the message goes in front of the fields that may have been recorded
        // before it
        if field.name() == "message" {
            let fields = std::mem::take(&mut self.0);
            let _ = write!(self.0, "{value:?}");
            if !fields.is_empty() {
                self.0.push(' ');
                self.0.push_str(&fields);
            }
        } else {
            if !self.0.is_empty() {
                self.0.push(' ');
            }
            let _ = write!(self.0, "{}={value:?}", field.name());
        }
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        // strings are written without the quotes `Debug` would add
        if field.name() == "message" {
            self.record_debug(field, &format_args!("{value}"));
        } else {
            if !self.0.is_empty() {
                self.0.push(' ');
            }
            let _ = write!(self.0, "{}={value}", field.name());
        }
    }
}

#[cfg(test)]
mod tests {
    use tracing_subscriber::layer::SubscriberExt;

    use super::*;

    /// The buffer is global: tests that read or clear it must NOT overlap.
    static BUFFER: Mutex<()> = Mutex::new(());

    // an event is stored with its level, message first and fields after
    #[test]
    fn captures_message_and_fields() {
        let _buffer = BUFFER.lock().unwrap();
        let subscriber = tracing_subscriber::registry().with(BufferLayer);
        tracing::subscriber::with_default(subscriber, || {
            tracing::warn!(interface = "submarine0", "routes applied");
        });
        let last = snapshot().pop().unwrap();
        assert_eq!(last.level, "warn");
        assert_eq!(last.message, "routes applied interface=submarine0");
    }

    // lines logged before `clear` are gone, later ones are kept
    #[test]
    fn clear_empties_the_buffer() {
        let _buffer = BUFFER.lock().unwrap();
        let subscriber = tracing_subscriber::registry().with(BufferLayer);
        tracing::subscriber::with_default(subscriber, || {
            tracing::info!("before");
            clear();
            tracing::info!("after");
        });
        let messages: Vec<String> = snapshot().into_iter().map(|l| l.message).collect();
        assert!(!messages.contains(&"before".to_owned()));
        assert!(messages.contains(&"after".to_owned()));
    }
}
