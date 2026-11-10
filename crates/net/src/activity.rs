//! In-flight request tracking, so the page layer can tell when the network
//! has gone idle.

use std::fmt;
use std::pin::pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use tokio::sync::watch;

/// Counts the requests of one browsing session that are in flight.
///
/// A request is in flight from the moment it is sent until its response body
/// has been read to the end, reading it failed, or the response was dropped.
/// Redirects are part of the same request. Requests refused by the policy are
/// never counted.
///
/// Clones share the same counter.
#[derive(Clone)]
pub struct NetworkActivity {
    inner: Arc<Inner>,
}

struct Inner {
    in_flight: watch::Sender<usize>,
    started: AtomicU64,
}

impl Default for NetworkActivity {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for NetworkActivity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("NetworkActivity")
            .field("in_flight", &self.in_flight())
            .field("started", &self.requests_started())
            .finish()
    }
}

impl NetworkActivity {
    /// A tracker with nothing in flight.
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Inner {
                in_flight: watch::Sender::new(0),
                started: AtomicU64::new(0),
            }),
        }
    }

    /// The number of requests in flight right now.
    pub fn in_flight(&self) -> usize {
        *self.inner.in_flight.borrow()
    }

    /// The number of requests started since the tracker was created.
    pub fn requests_started(&self) -> u64 {
        self.inner.started.load(Ordering::Relaxed)
    }

    /// A receiver that sees every change to the in-flight count.
    pub fn subscribe(&self) -> watch::Receiver<usize> {
        self.inner.in_flight.subscribe()
    }

    /// Waits until no more than `max_in_flight` requests have been in flight
    /// for `quiet` without interruption: the "network idle" condition
    /// (`max_in_flight` 0 is Puppeteer's `networkidle0`, 2 is
    /// `networkidle2`).
    ///
    /// Requests starting and finishing within the limit don't restart the
    /// quiet period; going over the limit does.
    pub async fn wait_for_idle(&self, max_in_flight: usize, quiet: Duration) {
        let mut rx = self.subscribe();
        loop {
            if rx.wait_for(|&n| n <= max_in_flight).await.is_err() {
                // The sender lives as long as `self`, so this can't happen.
                return;
            }
            let mut timer = pin!(tokio::time::sleep(quiet));
            loop {
                tokio::select! {
                    () = &mut timer => return,
                    changed = rx.changed() => {
                        if changed.is_err() {
                            return;
                        }
                        if *rx.borrow_and_update() > max_in_flight {
                            break;
                        }
                    }
                }
            }
        }
    }

    /// Marks a request as started. It stays in flight until the returned
    /// guard is dropped.
    pub fn start(&self) -> InFlight {
        self.inner.started.fetch_add(1, Ordering::Relaxed);
        self.inner.in_flight.send_modify(|n| *n += 1);
        InFlight {
            activity: self.clone(),
        }
    }
}

/// Keeps one request counted as in flight until dropped.
#[must_use = "the request stops counting as in flight when this is dropped"]
pub struct InFlight {
    activity: NetworkActivity,
}

impl fmt::Debug for InFlight {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("InFlight").finish_non_exhaustive()
    }
}

impl Drop for InFlight {
    fn drop(&mut self) {
        self.activity
            .inner
            .in_flight
            .send_modify(|n| *n = n.saturating_sub(1));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_guards() {
        let activity = NetworkActivity::new();
        assert_eq!(activity.in_flight(), 0);
        let a = activity.start();
        let b = activity.clone().start();
        assert_eq!(activity.in_flight(), 2);
        drop(a);
        assert_eq!(activity.in_flight(), 1);
        drop(b);
        assert_eq!(activity.in_flight(), 0);
        assert_eq!(activity.requests_started(), 2);
    }

    #[tokio::test(start_paused = true)]
    async fn idle_when_nothing_in_flight() {
        let activity = NetworkActivity::new();
        let start = tokio::time::Instant::now();
        activity.wait_for_idle(0, Duration::from_millis(500)).await;
        assert_eq!(start.elapsed(), Duration::from_millis(500));
    }

    #[tokio::test(start_paused = true)]
    async fn going_over_the_limit_restarts_the_quiet_period() {
        let activity = NetworkActivity::new();
        let guard = activity.start();
        let tracker = activity.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(100)).await;
            drop(guard);
            tokio::time::sleep(Duration::from_millis(300)).await;
            let again = tracker.start();
            tokio::time::sleep(Duration::from_millis(50)).await;
            drop(again);
        });
        let start = tokio::time::Instant::now();
        activity.wait_for_idle(0, Duration::from_millis(500)).await;
        // Idle from 100ms, interrupted at 400ms, idle again from 450ms.
        assert_eq!(start.elapsed(), Duration::from_millis(950));
    }

    #[tokio::test(start_paused = true)]
    async fn activity_within_the_limit_does_not_restart() {
        let activity = NetworkActivity::new();
        let tracker = activity.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(200)).await;
            let a = tracker.start();
            let b = tracker.start();
            tokio::time::sleep(Duration::from_millis(100)).await;
            drop((a, b));
        });
        let start = tokio::time::Instant::now();
        activity.wait_for_idle(2, Duration::from_millis(500)).await;
        assert_eq!(start.elapsed(), Duration::from_millis(500));
    }
}
