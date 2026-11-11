//! [`Service`]: admits jobs up to the concurrency cap and runs each on a
//! thread of its own.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use serde_json::json;
use tokio::sync::{Semaphore, oneshot};

use crate::job::Job;
use crate::log;
use crate::run::{self, Outcome, RunOptions};

/// Default for [`Config::max_concurrency`].
pub const DEFAULT_MAX_CONCURRENCY: usize = 4;
/// Default for [`Config::max_queue`].
pub const DEFAULT_MAX_QUEUE: usize = 32;
/// Default for [`Config::max_timeout`].
pub const DEFAULT_MAX_TIMEOUT: Duration = Duration::from_secs(300);
/// Default for [`Config::shutdown_timeout`]: Cloud Run stops an instance
/// 10 seconds after `SIGTERM`.
pub const DEFAULT_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(8);
/// The largest job accepted, in bytes.
pub const MAX_JOB_SIZE: usize = 1024 * 1024;

/// Stack of the threads jobs run on. Only the pages touched count towards
/// memory use; QuickJS stops scripts well before the end.
const JOB_STACK_SIZE: usize = 8 * 1024 * 1024;

/// How the service runs.
#[derive(Clone, Debug)]
pub struct Config {
    /// How many jobs run at once.
    pub max_concurrency: usize,
    /// How many more jobs may wait for a free slot; the rest are turned
    /// away with 503.
    pub max_queue: usize,
    /// The longest a job may take.
    pub max_timeout: Duration,
    /// If set, scrape requests must carry `Authorization: Bearer <token>`.
    pub auth_token: Option<String>,
    /// How long running jobs get to finish after a shutdown signal.
    pub shutdown_timeout: Duration,
    /// URL patterns every job refuses; the cloud metadata endpoints by
    /// default.
    pub blocked: Vec<String>,
    /// Whether to log a line to stderr per job.
    pub log: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            max_concurrency: DEFAULT_MAX_CONCURRENCY,
            max_queue: DEFAULT_MAX_QUEUE,
            max_timeout: DEFAULT_MAX_TIMEOUT,
            auth_token: None,
            shutdown_timeout: DEFAULT_SHUTDOWN_TIMEOUT,
            blocked: RunOptions::default().blocked,
            log: true,
        }
    }
}

/// Runs jobs, at most [`Config::max_concurrency`] at a time, each on its
/// own thread with its own single-threaded Tokio runtime, browser and
/// page. A page that spins in JavaScript holds up only its own job.
#[derive(Debug)]
pub struct Service {
    config: Config,
    /// Permits for running and waiting jobs together.
    admitted: Arc<Semaphore>,
    /// Permits for running jobs.
    slots: Arc<Semaphore>,
    running: Arc<AtomicUsize>,
    waiting: AtomicUsize,
    draining: AtomicBool,
}

impl Service {
    /// A service with `config`.
    pub fn new(config: Config) -> Self {
        let max_concurrency = config.max_concurrency.max(1);
        Self {
            admitted: Arc::new(Semaphore::new(
                max_concurrency.saturating_add(config.max_queue),
            )),
            slots: Arc::new(Semaphore::new(max_concurrency)),
            running: Arc::new(AtomicUsize::new(0)),
            waiting: AtomicUsize::new(0),
            draining: AtomicBool::new(false),
            config,
        }
    }

    /// The service's configuration.
    pub fn config(&self) -> &Config {
        &self.config
    }

    /// Runs `job` once a slot is free, or turns it away with 503 if too
    /// many jobs are already waiting or the service is shutting down.
    ///
    /// Dropping the returned future (when the client goes away) stops the
    /// job.
    pub async fn submit(&self, job: Job) -> Outcome {
        if self.is_draining() {
            return Outcome::error(503, "shutting down");
        }
        let Ok(admitted) = Arc::clone(&self.admitted).try_acquire_owned() else {
            return Outcome::error(503, "too many jobs; try again shortly");
        };
        let queued_at = Instant::now();
        let slot = {
            let _waiting = Counter::start(&self.waiting);
            Arc::clone(&self.slots).acquire_owned().await
        };
        let Ok(slot) = slot else {
            return Outcome::error(503, "shutting down");
        };
        let queued = queued_at.elapsed();

        let options = RunOptions {
            max_timeout: self.config.max_timeout,
            deadline: None,
            blocked: self.config.blocked.clone(),
        };
        let log = self.config.log;
        let running = Arc::clone(&self.running);
        let (tx, rx) = oneshot::channel();
        let thread = std::thread::Builder::new()
            .name("dasher-job".into())
            .stack_size(JOB_STACK_SIZE)
            .spawn(move || {
                // Held until the job's page and runtime are gone.
                let _permits = (admitted, slot);
                let _running = Counter::start(&running);
                run_on_own_runtime(&job, &options, queued, log, tx);
            });
        if let Err(e) = thread {
            return Outcome::error(503, format!("cannot start a job thread: {e}"));
        }
        rx.await
            .unwrap_or_else(|_| Outcome::error(500, "the job failed unexpectedly"))
    }

    /// Stops admitting jobs; the ones already admitted still run.
    pub fn start_draining(&self) {
        self.draining.store(true, Ordering::SeqCst);
    }

    /// Whether the service has stopped admitting jobs.
    pub fn is_draining(&self) -> bool {
        self.draining.load(Ordering::SeqCst)
    }

    /// How many jobs are running.
    pub fn running(&self) -> usize {
        self.running.load(Ordering::SeqCst)
    }

    /// How many jobs are waiting for a slot.
    pub fn waiting(&self) -> usize {
        self.waiting.load(Ordering::SeqCst)
    }

    /// The health report: `status` is `"ok"`, or `"draining"` while
    /// shutting down.
    pub fn health(&self) -> serde_json::Value {
        json!({
            "status": if self.is_draining() { "draining" } else { "ok" },
            "version": env!("CARGO_PKG_VERSION"),
            "running": self.running(),
            "waiting": self.waiting(),
            "max_concurrency": self.config.max_concurrency.max(1),
        })
    }
}

/// Runs the job on a new current-thread runtime, sending the outcome to
/// `tx`. Gives up as soon as `tx`'s receiver is dropped.
fn run_on_own_runtime(
    job: &Job,
    options: &RunOptions,
    queued: Duration,
    log: bool,
    mut tx: oneshot::Sender<Outcome>,
) {
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(e) => {
            let _ = tx.send(Outcome::error(500, format!("cannot start a runtime: {e}")));
            return;
        }
    };
    let outcome = runtime.block_on(async {
        tokio::select! {
            outcome = run::run(job, options, queued) => Some(outcome),
            () = tx.closed() => None,
        }
    });
    match outcome {
        Some(outcome) => {
            if log {
                log::job(job, &outcome);
            }
            let _ = tx.send(outcome);
        }
        None if log => log::job_abandoned(job),
        None => {}
    }
}

/// Counts something while alive.
struct Counter<'a>(&'a AtomicUsize);

impl<'a> Counter<'a> {
    fn start(counter: &'a AtomicUsize) -> Self {
        counter.fetch_add(1, Ordering::SeqCst);
        Self(counter)
    }
}

impl Drop for Counter<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}
