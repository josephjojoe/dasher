//! dasher as a service: scraping jobs in, JSON out.
//!
//! A [`Job`] names a URL, how to wait for it, and what to send back (the
//! HTML, values pulled out with selectors or JavaScript, or both).
//! [`run`] loads it in a fresh browser and page, which are dropped as soon
//! as the job is done, and gives an [`Outcome`]: an HTTP status and a
//! JSON body with the final URL, the site's status, the extracted values,
//! console errors and timings.

pub mod job;
pub mod report;
mod run;

pub use job::{InvalidJob, Job};
pub use run::{METADATA_BLOCKS, Outcome, RunOptions, run};
