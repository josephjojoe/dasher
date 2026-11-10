//! Driving the event loop asynchronously.

use std::time::{Duration, Instant};

use dasher_js::{ClockMode, JsRuntime, RunLimits, RunOutcome, RuntimeOptions};

fn runtime(clock: ClockMode) -> JsRuntime {
    JsRuntime::new(RuntimeOptions {
        clock,
        ..RuntimeOptions::default()
    })
    .unwrap()
}

fn json(js: &JsRuntime, expr: &str) -> String {
    js.eval_json(expr)
        .unwrap()
        .unwrap_or_else(|| "undefined".into())
}

const fn ms(n: u64) -> Duration {
    Duration::from_millis(n)
}

#[tokio::test]
async fn virtual_clock_fast_forwards_long_timers() {
    let js = runtime(ClockMode::Virtual);
    js.run_script(
        r#"
        var log = [];
        setTimeout(() => {
            log.push('ten seconds');
            requestAnimationFrame(() => log.push('frame'));
            Promise.resolve().then(() => setTimeout(() => log.push('a minute'), 50_000));
        }, 10_000);
        "#,
        "a.js",
    )
    .unwrap();
    let started = Instant::now();
    assert_eq!(js.run(RunLimits::default()).await, RunOutcome::Idle);
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "{:?}",
        started.elapsed()
    );
    assert_eq!(json(&js, "log"), r#"["ten seconds","frame","a minute"]"#);
    assert!(js.now() >= Duration::from_secs(60));
    assert!(json(&js, "performance.now()").parse::<f64>().unwrap() >= 60_000.0);
}

#[tokio::test]
async fn realtime_clock_waits_for_timers() {
    let js = runtime(ClockMode::Realtime);
    js.run_script(
        "var fired = 0; setTimeout(() => fired = performance.now(), 60);",
        "a.js",
    )
    .unwrap();
    let started = Instant::now();
    assert_eq!(js.run(RunLimits::default()).await, RunOutcome::Idle);
    assert!(started.elapsed() >= ms(60));
    assert!(json(&js, "fired").parse::<f64>().unwrap() >= 60.0);
}

#[tokio::test]
async fn until_bounds_page_time_and_leaves_later_work() {
    let js = runtime(ClockMode::Virtual);
    js.run_script("var n = 0; setInterval(() => n++, 1000);", "a.js")
        .unwrap();
    let outcome = js.run(RunLimits::until(js.now() + ms(5500))).await;
    assert_eq!(outcome, RunOutcome::ReachedTime);
    assert_eq!(json(&js, "n"), "5");
    assert!(js.now() >= ms(5500));
    assert_eq!(js.pending_timers(), 1);

    // Each further run covers another stretch of page time.
    js.run(RunLimits::until(js.now() + ms(2000))).await;
    assert_eq!(json(&js, "n"), "7");

    let js = runtime(ClockMode::Realtime);
    js.run_script("setTimeout(() => {}, 60_000)", "a.js")
        .unwrap();
    // Page time starts when the runtime is created, so measure from now.
    let started = Instant::now();
    assert_eq!(
        js.run(RunLimits::until(js.now() + ms(50))).await,
        RunOutcome::ReachedTime
    );
    assert!(started.elapsed() >= ms(40));
}

#[tokio::test]
async fn task_limit_stops_endless_animation() {
    let js = runtime(ClockMode::Manual);
    js.run_script(
        "(function loop() { requestAnimationFrame(loop); })();",
        "a.js",
    )
    .unwrap();
    let limits = RunLimits {
        max_tasks: Some(100),
        ..RunLimits::default()
    };
    assert_eq!(js.run(limits).await, RunOutcome::TaskLimit);
}

#[tokio::test]
async fn deadline_stops_a_run_waiting_on_timers() {
    let js = runtime(ClockMode::Realtime);
    js.run_script("setTimeout(() => {}, 60_000)", "a.js")
        .unwrap();
    js.set_deadline(Some(Instant::now() + ms(50)));
    let started = Instant::now();
    assert_eq!(
        js.run(RunLimits::default()).await,
        RunOutcome::DeadlineExceeded
    );
    assert!(started.elapsed() < Duration::from_secs(5));
}

#[tokio::test]
async fn deadline_stops_a_runaway_virtual_loop() {
    let js = runtime(ClockMode::Virtual);
    js.run_script("(function tick() { setTimeout(tick, 1); })();", "a.js")
        .unwrap();
    js.set_deadline(Some(Instant::now() + ms(100)));
    assert_eq!(
        js.run(RunLimits::default()).await,
        RunOutcome::DeadlineExceeded
    );
}

#[tokio::test]
async fn interrupt_handle_wakes_a_waiting_run() {
    let js = runtime(ClockMode::Realtime);
    js.run_script("setTimeout(() => {}, 60_000)", "a.js")
        .unwrap();
    let handle = js.interrupt_handle();
    std::thread::spawn(move || {
        std::thread::sleep(ms(50));
        handle.interrupt();
    });
    let started = Instant::now();
    assert_eq!(js.run(RunLimits::default()).await, RunOutcome::Interrupted);
    assert!(started.elapsed() < Duration::from_secs(5));
}

#[tokio::test]
async fn tasks_from_other_threads_are_awaited() {
    let js = runtime(ClockMode::Manual);
    js.run_script(
        "var results = []; globalThis.deliver = (v) => results.push(v);",
        "a.js",
    )
    .unwrap();
    let sender = js.task_sender();
    let op = sender.begin_operation();
    let worker = std::thread::spawn(move || {
        std::thread::sleep(ms(50));
        op.complete(|ctx| ctx.eval::<(), _>("deliver('from worker')"));
    });
    assert!(!js.is_idle());
    assert_eq!(js.run(RunLimits::default()).await, RunOutcome::Idle);
    worker.join().unwrap();
    assert_eq!(json(&js, "results"), r#"["from worker"]"#);
    assert!(js.is_idle());

    // A plain send also wakes the loop, and errors are captured.
    sender.send(|ctx| ctx.eval::<(), _>("throw new Error('bad delivery')"));
    assert_eq!(js.run(RunLimits::default()).await, RunOutcome::Idle);
    assert_eq!(js.take_errors()[0].message, "Error: bad delivery");
}

#[tokio::test]
async fn virtual_time_runs_in_real_time_while_host_work_is_pending() {
    let js = runtime(ClockMode::Virtual);
    js.run_script(
        r#"
        var log = [];
        globalThis.deliver = () => log.push('response');
        setTimeout(() => log.push('timeout'), 5000);
        setTimeout(() => log.push('tick'), 10);
        "#,
        "a.js",
    )
    .unwrap();
    let op = js.task_sender().begin_operation();
    let worker = std::thread::spawn(move || {
        std::thread::sleep(ms(100));
        op.complete(|ctx| ctx.eval::<(), _>("deliver()"));
    });
    assert_eq!(js.run(RunLimits::default()).await, RunOutcome::Idle);
    worker.join().unwrap();
    // The short timer fired in real time while the request was in flight;
    // the long one was only skipped to once nothing else was pending.
    assert_eq!(json(&js, "log"), r#"["tick","response","timeout"]"#);
}

#[tokio::test]
async fn abandoned_operations_do_not_keep_the_loop_alive() {
    let js = runtime(ClockMode::Manual);
    let op = js.task_sender().begin_operation();
    std::thread::spawn(move || {
        std::thread::sleep(ms(20));
        drop(op);
    });
    assert_eq!(js.run(RunLimits::default()).await, RunOutcome::Idle);
}

#[tokio::test]
async fn senders_outliving_the_runtime_are_harmless() {
    let js = runtime(ClockMode::Manual);
    let sender = js.task_sender();
    let op = sender.begin_operation();
    drop(js);
    sender.send(|ctx| ctx.eval::<(), _>("1"));
    op.complete(|ctx| ctx.eval::<(), _>("1"));
}
