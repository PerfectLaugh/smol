//! An I/O object for waking up threads blocked on the reactor.
//!
//! We use the self-pipe trick explained [here](https://cr.yp.to/docs/selfpipe.html).
//!
//! On Unix systems, the self-pipe is a pair of unnamed connected sockets. On Windows, the
//! self-pipe is a pair of TCP sockets connected over localhost.

use std::io;
use std::sync::atomic::{self, AtomicBool, Ordering};
use std::sync::Arc;

use std::task::{Poll, Waker};

use piper::Lock;

use crate::reactor::Reactor;

/// A self-pipe.
struct Inner {
    /// Set to `true` if notified.
    flag: AtomicBool,

    /// Task to wake
    task: Lock<Option<Waker>>,
}

/// A flag that that triggers an I/O event whenever it is set.
#[derive(Clone)]
pub(crate) struct IoEvent(Arc<Inner>);

impl IoEvent {
    /// Creates a new `IoEvent`.
    pub fn new() -> io::Result<IoEvent> {
        Ok(IoEvent(Arc::new(Inner {
            flag: AtomicBool::new(false),
            task: Lock::new(None),
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
                if let Some(mut task) = self.0.task.try_lock() {
                    if let Some(waker) = task.take() {
                        // Wake the task.
                        waker.wake();
                        // Wake the reactor.
                        Reactor::get().wake();
                    }
                }
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
        futures::future::poll_fn(|cx| {
            if !self.0.flag.load(Ordering::SeqCst) {
                let lock = self.0.task.try_lock();
                if let Some(mut task) = lock {
                    // Place task if no task is stored.
                    if task.is_none() {
                        task.replace(cx.waker().clone());
                    }
                }

                Poll::Pending
            } else {
                Poll::Ready(())
            }
        })
        .await
    }
}
