//! Driving the event loop asynchronously.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use dasher_js::{
    Budget, ClockMode, JsRuntime, NavigationKind, NavigationRequest, RunLimits, RunOutcome,
    RuntimeOptions, WorkLimits, thread_cpu_time,
};

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
async fn back_to_back_runs_of_slow_tasks_let_other_futures_progress() {
    let js = runtime(ClockMode::Virtual);
    // A scheduler that reposts itself forever, each task computing for 2 ms:
    // too few tasks in a run for the count of them to bring a yield.
    js.run_script(
        r#"
        const channel = new MessageChannel();
        channel.port1.onmessage = () => {
            const end = performance.now() + 2;
            while (performance.now() < end) {}
            channel.port2.postMessage(null);
        };
        channel.port2.postMessage(null);
        "#,
        "a.js",
    )
    .unwrap();
    let ran = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&ran);
    let other = tokio::spawn(async move { flag.store(true, Ordering::Relaxed) });
    // As settling does: one stretch of page time after another, with
    // nothing else awaited in between.
    for _ in 0..4 {
        assert_eq!(
            js.run(RunLimits::until(js.now() + ms(50))).await,
            RunOutcome::ReachedTime
        );
    }
    assert!(ran.load(Ordering::Relaxed), "the other task never ran");
    other.await.unwrap();
}

#[tokio::test]
async fn scheduled_refresh_becomes_a_navigation_on_the_page_clock() {
    let js = runtime(ClockMode::Virtual);
    js.run_script("var n = 0; setInterval(() => n++, 1000);", "a.js")
        .unwrap();
    js.schedule_refresh("https://example.com/next", ms(4500));
    let due = js.refresh_due().unwrap();
    assert!(!js.is_idle());
    let started = Instant::now();
    assert_eq!(
        js.run(RunLimits::default()).await,
        RunOutcome::NavigationRequested
    );
    assert!(started.elapsed() < Duration::from_secs(2));
    assert!(js.now() >= due);
    // Timers due before the refresh ran first, and none after.
    assert_eq!(json(&js, "n"), "4");
    assert_eq!(js.refresh_due(), None);
    assert_eq!(
        js.take_navigation_request(),
        Some(NavigationRequest {
            url: "https://example.com/next".into(),
            kind: NavigationKind::Replace,
        })
    );

    // Real time is waited for.
    let js = runtime(ClockMode::Realtime);
    js.schedule_refresh("https://example.com/next", ms(60));
    let started = Instant::now();
    assert_eq!(
        js.run(RunLimits::default()).await,
        RunOutcome::NavigationRequested
    );
    assert!(started.elapsed() >= ms(50));
}

#[tokio::test]
async fn scheduled_refresh_yields_to_an_earlier_navigation() {
    let js = runtime(ClockMode::Manual);
    js.schedule_refresh("https://example.com/refresh", ms(0));
    js.run_script("location.href = 'https://example.com/script'", "a.js")
        .unwrap();
    assert_eq!(
        js.take_navigation_request().unwrap().url,
        "https://example.com/script"
    );
    // The host dropped the script's request; the refresh still comes due.
    js.run_ready().unwrap();
    assert_eq!(
        js.take_navigation_request().unwrap().url,
        "https://example.com/refresh"
    );

    js.schedule_refresh("https://example.com/later", ms(100));
    js.run_script("location.href = 'https://example.com/script'", "a.js")
        .unwrap();
    js.advance(ms(200)).unwrap();
    assert_eq!(
        js.take_navigation_request().unwrap().url,
        "https://example.com/script"
    );
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

#[tokio::test]
async fn the_skip_limit_turns_an_endless_timer_into_real_time_waiting() {
    let js = runtime(ClockMode::Virtual);
    js.run_script("var ticks = 0; setInterval(() => ticks++, 100);", "a.js")
        .unwrap();
    js.set_skip_limit(Some(js.skipped() + ms(1000)));
    // Skips ten ticks' worth at once, then waits for each in real time.
    let started = Instant::now();
    let cpu = thread_cpu_time();
    let until = js.now() + ms(1300);
    assert_eq!(
        js.run(RunLimits::until(until)).await,
        RunOutcome::ReachedTime
    );
    let took = started.elapsed();
    assert!(took >= ms(250) && took < ms(1500), "{took:?}");
    assert!(thread_cpu_time() - cpu < ms(200));
    assert_eq!(js.skip_left(), Some(Duration::ZERO));
    let ticks: u32 = json(&js, "ticks").parse().unwrap();
    assert!((11..=14).contains(&ticks), "{ticks}");

    // Lifted, it skips again.
    js.set_skip_limit(None);
    let started = Instant::now();
    let until = js.now() + Duration::from_secs(60);
    assert_eq!(
        js.run(RunLimits::until(until)).await,
        RunOutcome::ReachedTime
    );
    assert!(started.elapsed() < ms(500));
}

#[tokio::test]
async fn under_the_manual_clock_the_skip_limit_ends_the_run() {
    let js = runtime(ClockMode::Manual);
    js.run_script("setInterval(() => {}, 100);", "a.js")
        .unwrap();
    js.set_skip_limit(Some(ms(250)));
    assert_eq!(
        js.run(RunLimits::default()).await,
        RunOutcome::BudgetExhausted(Budget::PageTime)
    );
    assert_eq!(js.now(), ms(250));
}

#[tokio::test]
async fn idle_callbacks_run_as_time_passes_but_do_not_keep_the_page_busy() {
    let js = runtime(ClockMode::Virtual);
    js.run_script(
        r#"
        var idle = 0, timer = false;
        (function again() { requestIdleCallback(() => { idle++; again(); }); })();
        setTimeout(() => timer = true, 1000);
        "#,
        "a.js",
    )
    .unwrap();
    assert!(!js.is_idle());
    let started = Instant::now();
    assert_eq!(js.run(RunLimits::default()).await, RunOutcome::Idle);
    assert!(started.elapsed() < ms(500), "{:?}", started.elapsed());
    assert_eq!(json(&js, "timer"), "true");
    // One right away, then one per 50 ms idle period until the timer.
    let idle: u32 = json(&js, "idle").parse().unwrap();
    assert!((19..=22).contains(&idle), "{idle}");
    assert!(js.is_idle());
    assert_eq!(js.pending_timers(), 0);

    // Page time moved on by the host still runs them.
    js.advance(ms(500)).unwrap();
    let more: u32 = json(&js, "idle").parse().unwrap();
    assert_eq!(more, idle + 10);
}

#[tokio::test]
async fn a_promise_waiting_on_an_idle_callback_still_settles() {
    let js = runtime(ClockMode::Virtual);
    js.run_script(
        "var done = false; new Promise((r) => requestIdleCallback(r)).then(() => done = true);",
        "a.js",
    )
    .unwrap();
    assert_eq!(
        js.run(RunLimits::until(js.now() + ms(50))).await,
        RunOutcome::Idle
    );
    assert_eq!(json(&js, "done"), "true");
}

#[tokio::test]
async fn the_cpu_limit_stops_a_busy_page_until_lifted() {
    let js = runtime(ClockMode::Virtual);
    js.run_script(
        r#"
        var runs = 0;
        (function work() {
            runs++;
            const end = Date.now() + 5;
            while (Date.now() < end) {}
            setTimeout(work, 0);
        })();
        "#,
        "a.js",
    )
    .unwrap();
    js.set_work_limits(WorkLimits::from_now(Some(ms(100)), None, 0));
    let started = Instant::now();
    assert_eq!(
        js.run(RunLimits::default()).await,
        RunOutcome::BudgetExhausted(Budget::Cpu)
    );
    assert!(started.elapsed() < Duration::from_secs(3));
    assert_eq!(js.budget_exhausted(), Some(Budget::Cpu));
    // No more JS meanwhile; lifting the limits lets it run again.
    assert!(js.eval_json("runs").is_err());
    js.set_work_limits(WorkLimits::default());
    let runs: u32 = json(&js, "runs").parse().unwrap();
    assert!((5..=100).contains(&runs), "{runs}");
}

#[tokio::test]
async fn the_end_of_the_wait_starts_no_task_and_interrupts_none() {
    let js = runtime(ClockMode::Realtime);
    js.run_script(
        r#"
        var log = [];
        setTimeout(() => {
            const end = performance.now() + 300;
            while (performance.now() < end) {}
            log.push('long');
        }, 50);
        setTimeout(() => log.push('next'), 60);
        setTimeout(() => log.push('late'), 5000);
        "#,
        "a.js",
    )
    .unwrap();
    js.set_work_limits(WorkLimits::default().with_wait(Some(ms(150))));
    let started = Instant::now();
    assert_eq!(
        js.run(RunLimits::default()).await,
        RunOutcome::BudgetExhausted(Budget::Wait)
    );
    // The task that was running at 150ms ran to its end at 350ms, and the
    // one due after it was not started.
    assert!(started.elapsed() >= ms(350));
    assert!(js.waited_out());
    assert_eq!(js.budget_exhausted(), None);
    assert_eq!(json(&js, "log.join()"), r#""long""#);
    // The host can still run JS, and the loop runs again with new limits.
    js.set_work_limits(WorkLimits::default().with_wait(Some(ms(200))));
    let started = Instant::now();
    assert_eq!(
        js.run(RunLimits::default()).await,
        RunOutcome::BudgetExhausted(Budget::Wait)
    );
    // It did not wait for the timer due in five seconds.
    let took = started.elapsed();
    assert!(took >= ms(200) && took < ms(2000), "{took:?}");
    assert_eq!(json(&js, "log.join()"), r#""long,next""#);
}

#[tokio::test]
async fn a_script_looping_forever_is_stopped_by_the_cpu_limit() {
    let js = runtime(ClockMode::Virtual);
    js.set_work_limits(WorkLimits::from_now(Some(ms(50)), None, 0));
    let started = Instant::now();
    assert!(js.run_script("for (;;) {}", "a.js").is_err());
    assert!(started.elapsed() < Duration::from_secs(3));
    assert_eq!(js.budget_exhausted(), Some(Budget::Cpu));
}
