//! The Lambda runtime, against a fake Runtime API.

mod common;

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use dasher_serve::{LambdaConfig, run_lambda};
use dasher_test_server::{FixtureResponse, FixtureServer, RecordedRequest};
use serde_json::{Value, json};

const NEXT: &str = "/2018-06-01/runtime/invocation/next";

type Events = Arc<Mutex<VecDeque<(String, Duration, Value)>>>;

/// A Runtime API that hands out `events` (request id, deadline from now,
/// payload) in turn, then fails, which ends the runtime's loop.
fn runtime_api(events: Vec<(&str, Duration, Value)>) -> FixtureServer {
    let ids: Vec<&str> = events.iter().map(|(id, ..)| *id).collect();
    let (api, queue) = runtime_api_for(&ids);
    let events = events
        .into_iter()
        .map(|(id, left, e)| (id.to_owned(), left, e));
    queue.lock().unwrap().extend(events);
    api
}

/// A Runtime API that answers for invocations `ids`, handing out the
/// events pushed to the queue it returns.
fn runtime_api_for(ids: &[&str]) -> (FixtureServer, Events) {
    let queue = Events::default();
    let events = Arc::clone(&queue);
    let mut builder = FixtureServer::builder().route(NEXT, move |_| {
        let Some((id, left, event)) = events.lock().unwrap().pop_front() else {
            return FixtureResponse::new(500).with_body("no more events");
        };
        let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap();
        let deadline = (now + left).as_millis().to_string();
        FixtureResponse::json(event.to_string())
            .with_header("Lambda-Runtime-Aws-Request-Id", &id)
            .with_header("Lambda-Runtime-Deadline-Ms", &deadline)
    });
    for id in ids {
        let response = format!("/2018-06-01/runtime/invocation/{id}/response");
        // A request id ending in "too-big" has its response refused, as
        // Lambda refuses payloads over 6 MB.
        let status = if id.ends_with("too-big") { 413 } else { 202 };
        builder = builder.route(&response, move |_| FixtureResponse::new(status));
        let error = format!("/2018-06-01/runtime/invocation/{id}/error");
        builder = builder.route(&error, |_| FixtureResponse::new(202));
    }
    (builder.start().unwrap(), queue)
}

/// The payload posted for `id`'s `kind` (`response` or `error`).
fn posted(api: &FixtureServer, id: &str, kind: &str) -> Option<Value> {
    let path = format!("/2018-06-01/runtime/invocation/{id}/{kind}");
    let requests: Vec<RecordedRequest> = api.requests();
    let request = requests.iter().find(|r| r.path == path)?;
    assert_eq!(request.method, "POST");
    Some(serde_json::from_slice(&request.body).unwrap())
}

fn quiet() -> LambdaConfig {
    LambdaConfig {
        log: false,
        ..LambdaConfig::default()
    }
}

#[tokio::test]
async fn runs_direct_and_http_invocations() {
    let site = common::site();
    let minute = Duration::from_secs(60);
    let job = json!({
        "url": site.url("/shop.html"),
        "extract": {"names": {"selector": ".product a", "all": true}},
    });
    let api = runtime_api(vec![
        ("direct", minute, job.clone()),
        ("invalid", minute, json!({"url": "u", "bogus": true})),
        // A function URL request (payload format 2.0)...
        (
            "url",
            minute,
            json!({
                "version": "2.0",
                "rawPath": "/",
                "requestContext": {"http": {"method": "POST", "path": "/"}},
                "body": job.to_string(),
                "isBase64Encoded": false,
            }),
        ),
        // ...with a base64 body, as API Gateway (format 1.0) may send.
        (
            "gateway",
            minute,
            json!({
                "httpMethod": "POST",
                "requestContext": {},
                "body": "eyJ1cmwiOiJmdHA6Ly94LyJ9", // {"url":"ftp://x/"}
                "isBase64Encoded": true,
            }),
        ),
        (
            "health",
            minute,
            json!({"requestContext": {"http": {"method": "GET"}}}),
        ),
    ]);

    let error = run_lambda(&api.addr().to_string(), quiet())
        .await
        .unwrap_err();
    assert!(error.to_string().contains("no more events"), "{error}");

    let direct = posted(&api, "direct", "response").unwrap();
    assert_eq!(direct["status"], 200, "{direct}");
    assert_eq!(direct["extracted"]["names"], json!(["Kettle", "Toaster"]));
    assert!(direct["timings"]["total_ms"].is_u64());

    let invalid = posted(&api, "invalid", "response").unwrap();
    assert!(invalid["error"].as_str().unwrap().contains("bogus"));

    let url = posted(&api, "url", "response").unwrap();
    assert_eq!(url["statusCode"], 200);
    assert_eq!(url["headers"]["content-type"], "application/json");
    let body: Value = serde_json::from_str(url["body"].as_str().unwrap()).unwrap();
    assert_eq!(body["extracted"]["names"], json!(["Kettle", "Toaster"]));

    let gateway = posted(&api, "gateway", "response").unwrap();
    assert_eq!(gateway["statusCode"], 400);
    assert!(gateway["body"].as_str().unwrap().contains("http(s)"));

    let health = posted(&api, "health", "response").unwrap();
    assert_eq!(health["statusCode"], 200);
    assert!(
        health["body"]
            .as_str()
            .unwrap()
            .contains(r#""status":"ok""#)
    );
}

#[tokio::test]
async fn the_invocation_deadline_bounds_the_job() {
    let site = common::site();
    // /slow.html takes a second; the invocation has two, less the time
    // kept back for answering.
    let api = runtime_api(vec![(
        "short",
        Duration::from_secs(2),
        json!({"url": site.url("/slow.html"), "timeout_ms": 60_000}),
    )]);
    let _ = run_lambda(&api.addr().to_string(), quiet()).await;
    let response = posted(&api, "short", "response").unwrap();
    assert!(
        response["error"].as_str().unwrap().contains("timed out"),
        "{response}"
    );
}

#[tokio::test]
async fn refused_responses_are_reported_as_errors() {
    let site = common::site();
    let api = runtime_api(vec![(
        "too-big",
        Duration::from_secs(60),
        json!({"url": site.url("/shop.html"), "wait": "load"}),
    )]);
    let _ = run_lambda(&api.addr().to_string(), quiet()).await;
    assert!(posted(&api, "too-big", "response").is_some());
    let error = posted(&api, "too-big", "error").unwrap();
    assert_eq!(error["errorType"], "Dasher.ResponseRejected");
    assert!(error["errorMessage"].as_str().unwrap().contains("413"));
}

#[tokio::test]
async fn pages_cannot_reach_the_runtime_api() {
    let (api, events) = runtime_api_for(&["sneaky"]);
    let address = api.addr().to_string();
    let job = json!({ "url": format!("http://{address}{NEXT}") });
    events
        .lock()
        .unwrap()
        .push_back(("sneaky".into(), Duration::from_secs(60), job));

    let _ = run_lambda(&address, quiet()).await;
    let response = posted(&api, "sneaky", "response").unwrap();
    assert!(
        response["error"]
            .as_str()
            .unwrap()
            .contains("blocked pattern"),
        "{response}"
    );
    // Only the runtime itself asked for invocations.
    assert_eq!(api.request_count_for(NEXT), 2);
}
