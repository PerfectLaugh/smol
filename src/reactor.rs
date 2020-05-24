//! The reactor notifying [`Async`][`crate::Async`] and [`Timer`][`crate::Timer`].
//!
//! There is a single global reactor that contains all registered I/O handles and timers. The
//! reactor is polled by the executor, i.e. the [`run()`][`crate::run()`] function.

#[cfg(not(any(
    target_os = "linux",     // epoll
    target_os = "android",   // epoll
    target_os = "illumos",   // epoll
    target_os = "macos",     // kqueue
    target_os = "ios",       // kqueue
    target_os = "freebsd",   // kqueue
    target_os = "netbsd",    // kqueue
    target_os = "openbsd",   // kqueue
    target_os = "dragonfly", // kqueue
    target_os = "windows",   // wepoll
)))]
compile_error!("reactor does not support this target OS");

use std::collections::BTreeMap;
use std::io;
use std::mem;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::task::Waker;
use std::time::{Duration, Instant};

use crossbeam::queue::ArrayQueue;
use once_cell::sync::Lazy;

use crate::io_event::IoEvent;

/// The reactor.
///
/// Every async I/O handle and every timer is registered here. Invocations of
/// [`run()`][`crate::run()`] poll the reactor to check for new events every now and then.
///
/// There is only one global instance of this type, accessible by [`Reactor::get()`].
pub(crate) struct Reactor {
    /// Instance to epoll/kqueue/iocp.
    sys: &'static nio::Proactor,

    /// Lock to proactor.
    sys_lock: piper::Lock<()>,

    /// An ordered map of registered timers.
    ///
    /// Timers are in the order in which they fire. The `usize` in this type is a timer ID used to
    /// distinguish timers that fire at the same time. The `Waker` represents the task awaiting the
    /// timer.
    timers: piper::Mutex<BTreeMap<(Instant, usize), Waker>>,

    /// A queue of timer operations (insert and remove).
    ///
    /// When inserting or removing a timer, we don't process it immediately - we just push it into
    /// this queue. Timers actually get processed when the queue fills up or the reactor is polled.
    timer_ops: ArrayQueue<TimerOp>,

    /// An I/O event that is triggered when a new timer is registered.
    ///
    /// The reason why this field is lazily created is because `IoEvent`s can be created only after
    /// the reactor is fully initialized.
    timer_event: Lazy<IoEvent>,
}

impl Reactor {
    /// Returns a reference to the reactor.
    pub fn get() -> &'static Reactor {
        static REACTOR: Lazy<Reactor> = Lazy::new(|| Reactor {
            sys: nio::Proactor::get(),
            sys_lock: piper::Lock::new(()),
            timers: piper::Mutex::new(BTreeMap::new()),
            timer_ops: ArrayQueue::new(1000),
            timer_event: Lazy::new(|| IoEvent::new().expect("cannot create an `IoEvent`")),
        });
        &REACTOR
    }

    /// Wake the system proactor.
    pub fn wake(&self) {
        // Test the lock and wake it if locking (means someone holding the proactor).
        if let Some(_) = self.try_lock() {
            return;
        }
        let _ = self.sys.wake();
    }

    /// Registers a timer in the reactor.
    ///
    /// Returns the inserted timer's ID.
    pub fn insert_timer(&self, when: Instant, waker: &Waker) -> usize {
        // Generate a new timer ID.
        static ID_GENERATOR: AtomicUsize = AtomicUsize::new(1);
        let id = ID_GENERATOR.fetch_add(1, Ordering::Relaxed);

        // Push an insert operation.
        while self
            .timer_ops
            .push(TimerOp::Insert(when, id, waker.clone()))
            .is_err()
        {
            // Fire timers to drain the queue.
            self.fire_timers();
        }

        // Notify that a timer was added.
        self.timer_event.notify();

        id
    }

    /// Deregisters a timer from the reactor.
    pub fn remove_timer(&self, when: Instant, id: usize) {
        // Push a remove operation.
        while self.timer_ops.push(TimerOp::Remove(when, id)).is_err() {
            // Fire timers to drain the queue.
            self.fire_timers();
        }
    }

    /// Attempts to lock the reactor.
    pub fn try_lock(&self) -> Option<ReactorLock<'_>> {
        let reactor = self;
        let guard = self.sys_lock.try_lock()?;
        Some(ReactorLock {
            reactor,
            _guard: guard,
        })
    }

    /// Locks the reactor.
    pub async fn lock(&self) -> ReactorLock<'_> {
        let reactor = self;
        let guard = self.sys_lock.lock().await;
        ReactorLock {
            reactor,
            _guard: guard,
        }
    }

    /// Fires ready timers.
    ///
    /// Returns the duration until the next timer before this method was called.
    fn fire_timers(&self) -> Option<Duration> {
        // Clear this event because we're about to fire timers.
        self.timer_event.clear();

        let mut timers = self.timers.lock();

        // Process timer operations, but no more than the queue capacity because otherwise we could
        // keep popping operations forever.
        for _ in 0..self.timer_ops.capacity() {
            match self.timer_ops.pop() {
                Ok(TimerOp::Insert(when, id, waker)) => {
                    timers.insert((when, id), waker);
                }
                Ok(TimerOp::Remove(when, id)) => {
                    timers.remove(&(when, id));
                }
                Err(_) => break,
            }
        }

        let now = Instant::now();

        // Split timers into ready and pending timers.
        let pending = timers.split_off(&(now, 0));
        let ready = mem::replace(&mut *timers, pending);

        // Calculate the duration until the next event.
        let dur = if ready.is_empty() {
            // Duration until the next timer.
            timers
                .keys()
                .next()
                .map(|(when, _)| when.saturating_duration_since(now))
        } else {
            // Timers are about to fire right now.
            Some(Duration::from_secs(0))
        };

        // Wake up tasks waiting on timers.
        for (_, waker) in ready {
            waker.wake();
        }

        dur
    }
}

/// A lock on the reactor.
pub(crate) struct ReactorLock<'a> {
    reactor: &'a Reactor,
    _guard: piper::LockGuard<()>,
}

impl ReactorLock<'_> {
    /// Processes ready events without blocking.
    pub fn poll(&mut self) -> io::Result<()> {
        self.react(false)
    }

    /// Blocks until at least one event is processed.
    pub fn wait(&mut self) -> io::Result<()> {
        self.react(true)
    }

    /// Processes new events, optionally blocking until the first event.
    fn react(&mut self, block: bool) -> io::Result<()> {
        // Fire timers and compute the timeout for blocking on I/O events.
        let next_timer = self.reactor.fire_timers();
        let timeout = if block {
            next_timer
        } else {
            Some(Duration::from_secs(0))
        };

        // Block on I/O events.
        println!("POLL WAITING");
        match self.reactor.sys.wait(8, timeout) {
            // The timeout was hit so fire ready timers.
            Ok(0) => {
                self.reactor.fire_timers();
                println!("SIZE: 0");
                Ok(())
            }

            // At least one I/O event occured.
            Ok(size) => {
                println!("SIZE: {}", size);
                Ok(())
            }

            // An actual error occureed.
            Err(err) => Err(err),
        }
    }
}

/// A single timer operation.
enum TimerOp {
    Insert(Instant, usize, Waker),
    Remove(Instant, usize),
}
