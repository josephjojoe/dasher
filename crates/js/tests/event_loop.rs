//! Timers, microtasks, animation frames and page time.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use dasher_js::rquickjs;
use dasher_js::{ClockMode, ErrorKind, JsRuntime, LoopHandle, RuntimeOptions, Stopped};

fn manual() -> JsRuntime {
    JsRuntime::new(RuntimeOptions {
        clock: ClockMode::Manual,
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

#[test]
fn tasks_microtasks_and_timers_run_in_html_order() {
    let js = manual();
    js.run_script(
        r#"
        var log = [];
        setTimeout(() => log.push('timeout'), 0);
        Promise.resolve().then(() => log.push('promise'));
        queueMicrotask(() => log.push('microtask'));
        log.push('script');
        "#,
        "a.js",
    )
    .unwrap();
    assert_eq!(json(&js, "log"), r#"["script","promise","microtask"]"#);
    assert_eq!(js.run_ready(), Ok(1));
    assert_eq!(
        json(&js, "log"),
        r#"["script","promise","microtask","timeout"]"#
    );
    assert!(js.is_idle());
}

#[test]
fn microtasks_queued_by_a_timer_run_before_the_next_timer() {
    let js = manual();
    js.run_script(
        r#"
        var log = [];
        setTimeout(() => { log.push('t1'); Promise.resolve().then(() => log.push('m1')); }, 0);
        setTimeout(() => log.push('t2'), 0);
        "#,
        "a.js",
    )
    .unwrap();
    js.run_ready().unwrap();
    assert_eq!(json(&js, "log"), r#"["t1","m1","t2"]"#);
}

#[test]
fn timers_fire_in_due_order_when_time_advances() {
    let js = manual();
    js.run_script(
        r#"
        var log = [];
        setTimeout((a, b) => log.push('c' + a + b), 300, 1, 2);
        setTimeout(() => log.push('a'), 100);
        setTimeout("log.push('b')", 200);
        "#,
        "a.js",
    )
    .unwrap();
    assert_eq!(js.run_ready(), Ok(0));
    assert_eq!(js.next_wakeup(), Some(ms(100)));
    assert_eq!(js.advance(ms(150)), Ok(1));
    assert_eq!(json(&js, "log"), r#"["a"]"#);
    assert_eq!(js.now(), ms(150));
    assert_eq!(js.advance(ms(10_000)), Ok(2));
    assert_eq!(json(&js, "log"), r#"["a","b","c12"]"#);
    assert_eq!(js.now(), ms(10_150));
}

#[test]
fn timers_see_the_page_clock_when_they_fire() {
    let js = manual();
    js.run_script(
        "var seen = []; for (const t of [50, 20, 1000]) setTimeout(() => seen.push(performance.now()), t);",
        "a.js",
    )
    .unwrap();
    js.advance(Duration::from_secs(5)).unwrap();
    assert_eq!(json(&js, "seen"), "[20,50,1000]");
}

#[test]
fn clear_timeout_and_interval() {
    let js = manual();
    js.run_script(
        r#"
        var log = [];
        const a = setTimeout(() => log.push('a'), 10);
        const b = setTimeout(() => log.push('b'), 10);
        clearTimeout(a);
        clearTimeout(undefined); clearTimeout('nonsense'); clearInterval(9999);
        let n = 0;
        const i = setInterval(() => { log.push('i' + ++n); if (n === 3) clearInterval(i); }, 100);
        // A timer cleared by an earlier callback due at the same time never runs.
        setTimeout(() => clearTimeout(c), 500);
        const c = setTimeout(() => log.push('c'), 500);
        "#,
        "a.js",
    )
    .unwrap();
    js.advance(Duration::from_secs(10)).unwrap();
    assert_eq!(json(&js, "log"), r#"["b","i1","i2","i3"]"#);
    assert!(js.is_idle());
}

#[test]
fn intervals_repeat_at_their_period() {
    let js = manual();
    js.run_script(
        "var at = []; setInterval(() => at.push(performance.now()), 250);",
        "a.js",
    )
    .unwrap();
    js.advance(ms(1000)).unwrap();
    assert_eq!(json(&js, "at"), "[250,500,750,1000]");
    assert_eq!(js.pending_timers(), 1);
}

#[test]
fn timer_ids_are_distinct_positive_integers() {
    let js = manual();
    assert_eq!(
        json(
            &js,
            "[setTimeout(() => {}), setInterval(() => {}, 5), setTimeout(() => {})]"
        ),
        "[1,2,3]"
    );
}

#[test]
fn nested_zero_delay_timers_are_clamped() {
    let js = manual();
    js.run_script(
        "var at = []; (function tick() { at.push(performance.now()); if (at.length < 8) setTimeout(tick, 0); })();",
        "a.js",
    )
    .unwrap();
    js.advance(ms(100)).unwrap();
    // The first call is not a timer; the timer created at nesting level 6
    // is the first one clamped.
    assert_eq!(json(&js, "at"), "[0,0,0,0,0,0,0,4]");
}

#[test]
fn huge_timeouts_wrap_like_browsers() {
    let js = manual();
    js.run_script(
        "var fired = false; setTimeout(() => fired = true, 2 ** 32 + 5);",
        "a.js",
    )
    .unwrap();
    assert_eq!(js.next_wakeup(), Some(ms(5)));
    js.run_script("setTimeout(() => {}, -100)", "b.js").unwrap();
    assert_eq!(js.next_wakeup(), Some(ms(0)));
}

#[test]
fn errors_in_callbacks_are_reported_and_the_loop_continues() {
    let js = manual();
    js.run_script(
        r#"
        var log = [];
        setTimeout(() => { throw new Error('timer'); }, 0);
        setTimeout(() => log.push('after'), 0);
        queueMicrotask(() => { throw new Error('micro'); });
        requestAnimationFrame(() => { throw new Error('frame'); });
        "#,
        "a.js",
    )
    .unwrap();
    js.advance(ms(100)).unwrap();
    assert_eq!(json(&js, "log"), r#"["after"]"#);
    let errors: Vec<_> = js
        .take_errors()
        .into_iter()
        .map(|e| (e.kind, e.message))
        .collect();
    assert_eq!(
        errors,
        [
            (ErrorKind::Uncaught, "Error: micro".to_owned()),
            (ErrorKind::Uncaught, "Error: timer".to_owned()),
            (ErrorKind::Uncaught, "Error: frame".to_owned()),
        ]
    );
}

#[test]
fn animation_frames_follow_the_frame_clock() {
    let js = JsRuntime::new(RuntimeOptions {
        clock: ClockMode::Manual,
        frame_interval: ms(10),
        ..RuntimeOptions::default()
    })
    .unwrap();
    js.run_script(
        r#"
        var frames = [];
        requestAnimationFrame(t => frames.push(['a', t]));
        requestAnimationFrame(t => {
            frames.push(['b', t]);
            requestAnimationFrame(t2 => frames.push(['c', t2]));
        });
        const cancelled = requestAnimationFrame(() => frames.push(['never']));
        cancelAnimationFrame(cancelled);
        "#,
        "a.js",
    )
    .unwrap();
    assert_eq!(js.run_ready(), Ok(0), "no frame before the first boundary");
    assert_eq!(js.next_wakeup(), Some(ms(10)));
    js.advance(ms(10)).unwrap();
    assert_eq!(json(&js, "frames"), r#"[["a",10],["b",10]]"#);
    js.advance(ms(100)).unwrap();
    assert_eq!(json(&js, "frames"), r#"[["a",10],["b",10],["c",20]]"#);
    assert!(js.is_idle());
}

#[test]
fn cancelling_a_later_callback_in_the_same_frame_skips_it() {
    let js = manual();
    js.run_script(
        r#"
        var ran = [];
        let second;
        requestAnimationFrame(() => { ran.push(1); cancelAnimationFrame(second); });
        second = requestAnimationFrame(() => ran.push(2));
        "#,
        "a.js",
    )
    .unwrap();
    js.advance(ms(100)).unwrap();
    assert_eq!(json(&js, "ran"), "[1]");
}

#[test]
fn animation_loops_advance_one_frame_per_step() {
    let js = manual();
    js.run_script(
        "var count = 0; (function loop() { count++; requestAnimationFrame(loop); })();",
        "a.js",
    )
    .unwrap();
    // The 60th boundary of the default 60 Hz clock is at 1000.00002 ms.
    js.advance(ms(1001)).unwrap();
    // 60 frames, plus the synchronous first call.
    assert_eq!(json(&js, "count"), "61");
}

#[test]
fn date_follows_the_page_clock() {
    let js = manual();
    let before = json(&js, "Date.now()").parse::<f64>().unwrap();
    js.advance(Duration::from_secs(3600)).unwrap();
    let after = json(&js, "Date.now()").parse::<f64>().unwrap();
    assert_eq!(after - before, 3_600_000.0);
    assert_eq!(json(&js, "new Date().getTime() === Date.now()"), "true");
    assert_eq!(
        json(&js, "new Date(0).toISOString()"),
        r#""1970-01-01T00:00:00.000Z""#
    );
    assert_eq!(json(&js, "new Date(2020, 0, 1).getFullYear()"), "2020");
    assert_eq!(json(&js, "new Date() instanceof Date"), "true");
    assert_eq!(json(&js, "typeof Date()"), r#""string""#);
    assert_eq!(json(&js, "Date.UTC(2000, 0) === 946684800000"), "true");
    assert_eq!(json(&js, "Date.prototype.constructor === Date"), "true");
    assert_eq!(
        json(
            &js,
            "class D extends Date { y() { return this.getUTCFullYear(); } }; [new D(0).y(), new D() instanceof D]"
        ),
        "[1970,true]"
    );
    let origin = json(&js, "performance.timeOrigin").parse::<f64>().unwrap();
    assert!((before - origin).abs() < 1.0);
}

#[test]
fn performance_now_tracks_real_time_in_realtime_mode() {
    let js = JsRuntime::new(RuntimeOptions::default()).unwrap();
    let a = json(&js, "performance.now()").parse::<f64>().unwrap();
    std::thread::sleep(ms(20));
    let b = json(&js, "performance.now()").parse::<f64>().unwrap();
    assert!(b - a >= 20.0, "{a} -> {b}");
}

#[test]
fn realtime_timers_wait_for_the_wall_clock() {
    let js = JsRuntime::new(RuntimeOptions::default()).unwrap();
    js.run_script(
        "var fired = false; setTimeout(() => fired = true, 30);",
        "a.js",
    )
    .unwrap();
    assert_eq!(js.run_ready(), Ok(0));
    std::thread::sleep(ms(40));
    assert_eq!(js.run_ready(), Ok(1));
    assert_eq!(json(&js, "fired"), "true");
}

#[test]
fn host_tasks_run_in_queue_order_with_timers() {
    let js = manual();
    let seen = Rc::new(RefCell::new(Vec::new()));
    js.run_script(
        "var log = []; setTimeout(() => log.push('timer'), 0);",
        "a.js",
    )
    .unwrap();
    let s = Rc::clone(&seen);
    js.queue_task(move |ctx| {
        s.borrow_mut().push("host");
        ctx.eval::<(), _>("log.push('host'); Promise.resolve().then(() => log.push('host-micro'))")
    });
    js.queue_task(|ctx| ctx.eval::<(), _>("throw new Error('from host task')"));
    js.run_ready().unwrap();
    assert_eq!(*seen.borrow(), ["host"]);
    // The timer became runnable when the loop first looked, after the host
    // tasks had been queued.
    assert_eq!(json(&js, "log"), r#"["host","host-micro","timer"]"#);
    let errors = js.take_errors();
    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0].message, "Error: from host task");
}

#[test]
fn loop_handle_queues_tasks_from_native_functions() {
    let js = manual();
    js.with(|ctx| {
        let f = rquickjs::Function::new(ctx.clone(), |ctx: rquickjs::Ctx<'_>| {
            let handle = LoopHandle::from_ctx(&ctx).unwrap();
            handle.queue_task(|ctx| ctx.eval::<(), _>("globalThis.later = true"));
        })
        .unwrap();
        ctx.globals().set("defer", f).unwrap();
    });
    js.run_script("defer(); var now = typeof later;", "a.js")
        .unwrap();
    assert_eq!(json(&js, "now"), r#""undefined""#);
    js.run_ready().unwrap();
    assert_eq!(json(&js, "later"), "true");
}

#[test]
fn deadline_stops_the_loop() {
    let js = manual();
    js.run_script(
        "setInterval(() => { const end = Date.now(); while (true) {} }, 10);",
        "a.js",
    )
    .unwrap();
    js.set_deadline(Some(Instant::now() + ms(50)));
    assert_eq!(
        js.advance(Duration::from_secs(60)),
        Err(Stopped::DeadlineExceeded)
    );
    assert_eq!(js.take_errors()[0].kind, ErrorKind::Interrupted);

    let handle = js.interrupt_handle();
    js.set_deadline(None);
    handle.interrupt();
    assert_eq!(js.run_ready(), Err(Stopped::Interrupted));
}

#[test]
fn dropping_with_pending_work_is_clean() {
    for _ in 0..10 {
        let js = manual();
        js.run_script(
            "setTimeout(() => {}, 1e6); setInterval(() => {}, 5); requestAnimationFrame(() => {}); queueMicrotask(() => {}); new Promise(() => {});",
            "a.js",
        )
        .unwrap();
        js.with(|ctx| {
            let obj = rquickjs::Object::new(ctx.clone()).unwrap();
            let held = rquickjs::Persistent::save(&ctx, obj);
            js.queue_task(move |ctx| {
                let _ = held.restore(&ctx)?;
                Ok(())
            });
        });
    }
}
