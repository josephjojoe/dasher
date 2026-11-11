//! One-line JSON logs on stderr, which Cloud Logging and CloudWatch both
//! read: `severity` and `message`, plus fields.

use std::io::Write;

use serde_json::{Map, Value, json};

use crate::job::Job;
use crate::run::Outcome;

/// Logs `message` at `severity` (`"INFO"`, `"WARNING"`, `"ERROR"`) with
/// the fields of `fields`, an object.
pub fn log(severity: &str, message: &str, fields: Value) {
    let mut line = Map::new();
    line.insert("severity".into(), severity.into());
    line.insert("message".into(), message.into());
    if let Value::Object(fields) = fields {
        line.extend(fields);
    }
    let mut text = Value::Object(line).to_string();
    text.push('\n');
    // Losing a log line is better than failing the job.
    let _ = std::io::stderr().lock().write_all(text.as_bytes());
}

/// Logs how a job went.
pub fn job(job: &Job, outcome: &Outcome) {
    let body = &outcome.body;
    let timings = &body["timings"];
    let severity = match outcome.status {
        200 => "INFO",
        400..=499 => "WARNING",
        _ => "ERROR",
    };
    log(
        severity,
        "job finished",
        json!({
            "url": job.url,
            "final_url": body.get("url"),
            "result": outcome.status,
            "status": body.get("status"),
            "timed_out": body.get("timed_out"),
            "error": body.get("error"),
            "queued_ms": timings.get("queued_ms"),
            "total_ms": timings.get("total_ms"),
        }),
    );
}

/// Logs a job dropped because its client went away.
pub fn job_abandoned(job: &Job) {
    log(
        "WARNING",
        "job abandoned: the client went away",
        json!({ "url": job.url }),
    );
}
