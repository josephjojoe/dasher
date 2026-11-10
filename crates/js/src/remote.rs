//! Host work that finishes off the JS thread, such as network requests,
//! posting its results back to the event loop.

use std::collections::VecDeque;
use std::fmt;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use rquickjs::Ctx;
use tokio::sync::Notify;

/// A [`Task`](crate::Task) that can be created on another thread.
pub type SendTask = Box<dyn for<'js> FnOnce(Ctx<'js>) -> rquickjs::Result<()> + Send>;

/// State shared between a runtime and its senders and interrupt handles,
/// possibly across threads.
#[derive(Default)]
pub(crate) struct Remote {
    queue: Mutex<VecDeque<SendTask>>,
    /// Operations begun and not yet completed or dropped.
    pending: AtomicUsize,
    /// Set when the runtime is dropped; later tasks are discarded.
    closed: AtomicBool,
    /// Set by an `InterruptHandle`.
    pub(crate) kill: AtomicBool,
    /// Wakes the event loop when a task arrives, an operation ends or an
    /// interrupt is requested.
    pub(crate) notify: Notify,
}

impl Remote {
    fn queue(&self) -> MutexGuard<'_, VecDeque<SendTask>> {
        // A task never runs while the lock is held, so a poisoned lock still
        // guards a consistent queue.
        self.queue.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn push(&self, task: SendTask) {
        if !self.closed.load(Ordering::SeqCst) {
            self.queue().push_back(task);
        }
    }

    /// Takes every task that has arrived, in arrival order.
    pub(crate) fn drain(&self) -> VecDeque<SendTask> {
        std::mem::take(&mut *self.queue())
    }

    /// Whether host work that will post a task is still running. Read this
    /// before checking the queue: operations enqueue their final task before
    /// they stop counting as pending.
    pub(crate) fn has_pending(&self) -> bool {
        self.pending.load(Ordering::SeqCst) > 0
    }

    pub(crate) fn has_queued(&self) -> bool {
        !self.queue().is_empty()
    }

    pub(crate) fn interrupt(&self) {
        self.kill.store(true, Ordering::SeqCst);
        self.notify.notify_one();
    }

    /// Stops accepting tasks and drops the queued ones.
    pub(crate) fn close(&self) {
        self.closed.store(true, Ordering::SeqCst);
        self.queue().clear();
    }
}

/// Posts tasks to a runtime's event loop from any thread.
///
/// Tasks must be `Send`, so they cannot capture JS values; keep those on the
/// JS side (for example in a map keyed by request id) and send the id.
/// Tasks sent after the runtime is dropped are discarded.
#[derive(Clone)]
pub struct TaskSender {
    remote: Arc<Remote>,
}

impl fmt::Debug for TaskSender {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TaskSender").finish_non_exhaustive()
    }
}

impl TaskSender {
    pub(crate) fn new(remote: Arc<Remote>) -> Self {
        Self { remote }
    }

    /// Queues `task` to run as its own task on the event loop.
    pub fn send(
        &self,
        task: impl for<'js> FnOnce(Ctx<'js>) -> rquickjs::Result<()> + Send + 'static,
    ) {
        self.remote.push(Box::new(task));
        self.remote.notify.notify_one();
    }

    /// Registers host work in progress. Until the returned operation is
    /// completed or dropped, the event loop knows more work is coming: it
    /// does not report itself idle, and in [`ClockMode::Virtual`] it lets
    /// page time run at wall-clock speed instead of skipping ahead, so
    /// timeouts racing the work do not fire early.
    ///
    /// [`ClockMode::Virtual`]: crate::ClockMode::Virtual
    pub fn begin_operation(&self) -> PendingOperation {
        self.remote.pending.fetch_add(1, Ordering::SeqCst);
        PendingOperation {
            remote: Some(Arc::clone(&self.remote)),
        }
    }
}

/// Host work in progress, from [`TaskSender::begin_operation`].
///
/// Finish it with [`complete`](Self::complete), or drop it to abandon the
/// work without running anything.
#[must_use = "dropping a PendingOperation ends it immediately"]
pub struct PendingOperation {
    remote: Option<Arc<Remote>>,
}

impl fmt::Debug for PendingOperation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PendingOperation").finish_non_exhaustive()
    }
}

impl PendingOperation {
    /// Ends the operation, queueing `task` to deliver its result.
    pub fn complete(
        mut self,
        task: impl for<'js> FnOnce(Ctx<'js>) -> rquickjs::Result<()> + Send + 'static,
    ) {
        if let Some(remote) = self.remote.take() {
            remote.push(Box::new(task));
            finish(&remote);
        }
    }
}

impl Drop for PendingOperation {
    fn drop(&mut self) {
        if let Some(remote) = self.remote.take() {
            finish(&remote);
        }
    }
}

fn finish(remote: &Remote) {
    remote.pending.fetch_sub(1, Ordering::SeqCst);
    remote.notify.notify_one();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn operations_count_as_pending_until_completed_or_dropped() {
        let remote = Arc::new(Remote::default());
        let sender = TaskSender::new(Arc::clone(&remote));
        let a = sender.begin_operation();
        let b = sender.begin_operation();
        assert!(remote.has_pending());
        a.complete(|_| Ok(()));
        assert!(remote.has_pending());
        assert_eq!(remote.drain().len(), 1);
        drop(b);
        assert!(!remote.has_pending());
        assert!(!remote.has_queued());
    }

    #[test]
    fn closed_remotes_discard_tasks() {
        let remote = Arc::new(Remote::default());
        let sender = TaskSender::new(Arc::clone(&remote));
        sender.send(|_| Ok(()));
        remote.close();
        assert!(!remote.has_queued());
        sender.send(|_| Ok(()));
        sender.begin_operation().complete(|_| Ok(()));
        assert!(!remote.has_queued());
        assert!(!remote.has_pending());
    }

    #[test]
    fn senders_work_across_threads() {
        let remote = Arc::new(Remote::default());
        let sender = TaskSender::new(Arc::clone(&remote));
        let op = sender.begin_operation();
        std::thread::spawn(move || op.complete(|_| Ok(())))
            .join()
            .unwrap();
        assert!(!remote.has_pending());
        assert_eq!(remote.drain().len(), 1);
    }
}
