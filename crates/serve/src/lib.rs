//! dasher as a service: scraping jobs in, JSON out.
//!
//! A [`Job`] names a URL, how to wait for it, and what to send back (the
//! HTML, values pulled out with selectors or JavaScript, or both).
//! [`run`] loads it in a fresh browser and page, which are dropped as soon
//! as the job is done, and gives an [`Outcome`]: an HTTP status and a
//! JSON body with the final URL, the site's status, the extracted values,
//! console errors and timings.
//!
//! [`Server`] serves jobs over HTTP for Cloud Run and other container
//! platforms: `POST /scrape` and `GET /health`, with a concurrency cap
//! and graceful shutdown on `SIGTERM`. Each job runs on a thread of its
//! own through [`Service`].

mod http;
pub mod job;
mod log;
pub mod report;
mod run;
mod service;

pub use http::{Server, shutdown_signal};
pub use job::{InvalidJob, Job};
pub use run::{METADATA_BLOCKS, Outcome, RunOptions, run};
pub use service::{
    Config, DEFAULT_MAX_CONCURRENCY, DEFAULT_MAX_QUEUE, DEFAULT_MAX_TIMEOUT,
    DEFAULT_SHUTDOWN_TIMEOUT, MAX_JOB_SIZE, Service,
};
