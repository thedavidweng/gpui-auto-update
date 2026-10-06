//! Fan-out of Sparkle events from the main thread to waiting checks,
//! staging calls, and the state tracker.

use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};
use std::time::Duration;

use crate::event::{SparkleEvent, SparkleUpdate, UserChoice};

/// The channel through which an engine reports [`SparkleEvent`]s to the
/// backend.
///
/// The real engine publishes from Sparkle's delegate on the main thread;
/// test engines may publish from any thread, including from inside an engine
/// call. Cloning is cheap and every clone refers to the same channel.
#[derive(Clone, Default)]
pub struct SparkleEvents {
    inner: Arc<Mutex<Hub>>,
}

#[derive(Default)]
struct Hub {
    next_id: u64,
    subscribers: Vec<(u64, Sender<SparkleEvent>)>,
    pending: Option<SparkleUpdate>,
}

impl SparkleEvents {
    /// A channel with no subscribers.
    pub fn new() -> Self {
        Self::default()
    }

    /// Delivers `event` to everything currently waiting on Sparkle.
    pub fn publish(&self, event: SparkleEvent) {
        tracing::debug!(?event, "Sparkle event");
        let mut hub = self.lock();
        match &event {
            SparkleEvent::UpdateFound(update) => hub.pending = Some(update.clone()),
            SparkleEvent::NoUpdateFound(_)
            | SparkleEvent::WillRelaunch
            | SparkleEvent::SessionWillFinish
            | SparkleEvent::UserChoice {
                choice: UserChoice::Skip,
                ..
            } => hub.pending = None,
            _ => {}
        }
        hub.subscribers
            .retain(|(_, subscriber)| subscriber.send(event.clone()).is_ok());
    }

    /// The update Sparkle most recently found, while Sparkle may still be
    /// presenting or preparing it.
    pub fn pending_update(&self) -> Option<SparkleUpdate> {
        self.lock().pending.clone()
    }

    /// Starts receiving every event published from now on.
    pub(crate) fn subscribe(&self) -> EventStream {
        let (sender, receiver) = mpsc::channel();
        let mut hub = self.lock();
        hub.next_id += 1;
        let id = hub.next_id;
        hub.subscribers.push((id, sender));
        EventStream {
            id,
            receiver,
            hub: Arc::downgrade(&self.inner),
        }
    }

    fn lock(&self) -> MutexGuard<'_, Hub> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl std::fmt::Debug for SparkleEvents {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let hub = self.lock();
        f.debug_struct("SparkleEvents")
            .field("subscribers", &hub.subscribers.len())
            .field("pending", &hub.pending)
            .finish()
    }
}

/// Events published after [`SparkleEvents::subscribe`]; unsubscribes when
/// dropped.
pub(crate) struct EventStream {
    id: u64,
    receiver: Receiver<SparkleEvent>,
    // Weak so that a long-lived stream (the state tracker) does not keep the
    // channel alive: dropping the backend ends the stream.
    hub: Weak<Mutex<Hub>>,
}

impl EventStream {
    /// The next event, or `None` once the channel is gone.
    pub(crate) fn recv(&self) -> Option<SparkleEvent> {
        self.receiver.recv().ok()
    }

    /// The next event within `timeout`.
    pub(crate) fn recv_timeout(&self, timeout: Duration) -> Result<SparkleEvent, RecvTimeoutError> {
        self.receiver.recv_timeout(timeout)
    }
}

impl Drop for EventStream {
    fn drop(&mut self) {
        if let Some(hub) = self.hub.upgrade() {
            hub.lock()
                .unwrap_or_else(PoisonError::into_inner)
                .subscribers
                .retain(|(id, _)| *id != self.id);
        }
    }
}
