//! An I/O object for waking up threads blocked on the reactor.
//!
//! We use the self-pipe trick explained [here](https://cr.yp.to/docs/selfpipe.html).
//!
//! On Unix systems, the self-pipe is a pair of unnamed connected sockets. On Windows, the
//! self-pipe is a pair of TCP sockets connected over localhost.

use std::io;
use std::sync::atomic::{self, AtomicBool, Ordering};
use std::sync::Arc;

use futures::channel::mpsc;
use futures::StreamExt;

use crate::reactor::Reactor;

/// A self-pipe.
struct Inner {
    /// Set to `true` if notified.
    flag: AtomicBool,

    /// The writer side.
    writer: piper::Mutex<mpsc::Sender<()>>,

    /// The reader side, filled by `notify()`.
    reader: piper::Mutex<mpsc::Receiver<()>>,
}

/// A flag that that triggers an I/O event whenever it is set.
#[derive(Clone)]
pub(crate) struct IoEvent(Arc<Inner>);

impl IoEvent {
    /// Creates a new `IoEvent`.
    pub fn new() -> io::Result<IoEvent> {
        let (writer, reader) = mpsc::channel(1);

        Ok(IoEvent(Arc::new(Inner {
            flag: AtomicBool::new(false),
            writer: piper::Mutex::new(writer),
            reader: piper::Mutex::new(reader),
        })))
    }

    /// Sets the flag to `true`.
    pub fn notify(&self) {
        // Publish all in-memory changes before setting the flag.
        atomic::fence(Ordering::SeqCst);

        // If the flag is not set...
        if !self.0.flag.load(Ordering::SeqCst) {
            // If this thread sets it...
            if !self.0.flag.swap(true, Ordering::SeqCst) {
                // Trigger an I/O event and ignore the error.
                let mut writer = self.0.writer.lock();
                let _ = writer.try_send(());

                // Wake the reactor.
                Reactor::get().wake();
            }
        }
    }

    /// Sets the flag to `false`.
    pub fn clear(&self) -> bool {
        let value = self.0.flag.swap(false, Ordering::SeqCst);

        // Publish all in-memory changes after clearing the flag.
        atomic::fence(Ordering::SeqCst);
        value
    }

    /// Waits until notified.
    ///
    /// You should assume notifications may spuriously occur.
    pub async fn notified(&self) {
        let mut reader = self.0.reader.lock();
        reader.next().await;
    }
}
