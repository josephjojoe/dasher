//! ReadableStream, WritableStream, TransformStream and the text streams.

use dasher_js::{ClockMode, JsRuntime, RunLimits, RunOutcome, RuntimeOptions};

fn runtime() -> JsRuntime {
    JsRuntime::new(RuntimeOptions {
        clock: ClockMode::Virtual,
        ..RuntimeOptions::default()
    })
    .unwrap()
}

/// Runs `script`, which stores its results in `out`, then the event loop
/// until the page is idle, and returns `out` as JSON.
async fn run(script: &str) -> String {
    let js = runtime();
    js.run_script(&format!("var out = {{}};\n{script}"), "test.js")
        .unwrap();
    assert_eq!(js.run(RunLimits::default()).await, RunOutcome::Idle);
    assert_eq!(js.take_errors(), []);
    js.eval_json("out").unwrap().unwrap()
}

#[tokio::test]
async fn a_readable_stream_hands_out_enqueued_chunks_in_order() {
    let out = run(r#"
        let controller;
        const stream = new ReadableStream({ start(c) { controller = c; } });
        out.tag = Object.prototype.toString.call(stream);
        out.desired = controller.desiredSize;
        controller.enqueue('a');
        out.queued = controller.desiredSize;
        const reader = stream.getReader();
        out.locked = stream.locked;
        (async () => {
            const chunks = [];
            const first = await reader.read();
            chunks.push(first.value);
            // A read waiting on an empty queue is fulfilled by the next
            // enqueue.
            const pending = reader.read();
            controller.enqueue('b');
            chunks.push((await pending).value);
            controller.close();
            out.end = await reader.read();
            await reader.closed;
            out.chunks = chunks;
        })();
    "#)
    .await;
    assert_eq!(
        out,
        r#"{"tag":"[object ReadableStream]","desired":1,"queued":0,"locked":true,"end":{"done":true},"chunks":["a","b"]}"#
    );
}

#[tokio::test]
async fn pull_is_called_as_the_reader_asks_and_backpressure_holds_it() {
    let out = run(r#"
        let n = 0;
        const pulls = [];
        const stream = new ReadableStream({
            pull(c) {
                pulls.push(c.desiredSize);
                n += 1;
                if (n > 3) c.close(); else c.enqueue(n);
            },
        }, new CountQueuingStrategy({ highWaterMark: 1 }));
        (async () => {
            await null; await null;
            // Only the high-water mark's worth was pulled ahead.
            out.ahead = pulls.length;
            const got = [];
            for await (const chunk of stream) got.push(chunk);
            out.got = got;
            out.pulls = pulls.length;
            out.locked = stream.locked;
        })();
    "#)
    .await;
    assert_eq!(out, r#"{"ahead":1,"got":[1,2,3],"pulls":4,"locked":false}"#);
}

#[tokio::test]
async fn errors_and_cancellation_reach_readers_and_sources() {
    let out = run(r#"
        let controller;
        const failing = new ReadableStream({ start(c) { controller = c; } });
        const reader = failing.getReader();
        reader.read().catch((e) => { out.readError = e.message; });
        reader.closed.catch((e) => { out.closedError = e.message; });
        controller.error(new Error('boom'));
        out.desired = controller.desiredSize;

        const throwing = new ReadableStream({ start() { throw new TypeError('bad start'); } });
        throwing.getReader().read().catch((e) => { out.startError = e.name; });

        const cancelled = new ReadableStream({
            start(c) { c.enqueue(1); },
            cancel(reason) { out.cancelReason = reason; },
        });
        cancelled.cancel('stop').then((v) => { out.cancelResult = v === undefined; });

        // A locked stream cannot be cancelled directly.
        const locked = new ReadableStream();
        locked.getReader();
        locked.cancel().catch((e) => { out.lockedCancel = e.name; });
        try { locked.getReader(); } catch (e) { out.secondReader = e.name; }
        try { new ReadableStream().getReader({ mode: 'byob' }); } catch (e) { out.byob = e.name; }
    "#)
    .await;
    assert_eq!(
        out,
        r#"{"desired":null,"cancelReason":"stop","secondReader":"TypeError","byob":"TypeError","closedError":"boom","readError":"boom","startError":"TypeError","lockedCancel":"TypeError","cancelResult":true}"#
    );
}

#[tokio::test]
async fn releasing_a_reader_unlocks_the_stream() {
    let out = run(r#"
        const stream = new ReadableStream({ start(c) { c.enqueue('x'); c.enqueue('y'); c.close(); } });
        const first = stream.getReader();
        (async () => {
            out.first = (await first.read()).value;
            first.releaseLock();
            out.unlocked = !stream.locked;
            first.read().catch((e) => { out.released = e.name; });
            const second = stream.getReader();
            out.second = (await second.read()).value;
            out.done = (await second.read()).done;
        })();
    "#)
    .await;
    assert_eq!(
        out,
        r#"{"first":"x","unlocked":true,"released":"TypeError","second":"y","done":true}"#
    );
}

#[tokio::test]
async fn tee_gives_both_branches_every_chunk() {
    let out = run(r#"
        const source = ReadableStream.from(['a', 'b', 'c']);
        const [left, right] = source.tee();
        (async () => {
            const read = async (s) => { const got = []; for await (const c of s) got.push(c); return got; };
            const [l, r] = await Promise.all([read(left), read(right)]);
            out.left = l;
            out.right = r;
        })();
    "#)
    .await;
    assert_eq!(out, r#"{"left":["a","b","c"],"right":["a","b","c"]}"#);
}

#[tokio::test]
async fn from_accepts_sync_and_async_iterables() {
    let out = run(r#"
        async function* gen() { yield 1; yield 2; }
        (async () => {
            const got = [];
            for await (const c of ReadableStream.from(gen())) got.push(c);
            for await (const c of ReadableStream.from(new Set([3, 4]))) got.push(c);
            out.got = got;
            // Breaking out of the loop cancels the stream.
            let cancelled = false;
            const s = new ReadableStream({
                pull(c) { c.enqueue(0); },
                cancel() { cancelled = true; },
            });
            for await (const c of s) break;
            out.cancelled = cancelled;
        })();
    "#)
    .await;
    assert_eq!(out, r#"{"got":[1,2,3,4],"cancelled":true}"#);
}

#[tokio::test]
async fn a_writable_stream_writes_in_order_and_closes() {
    let out = run(r#"
        const written = [];
        const stream = new WritableStream({
            write(chunk) {
                return new Promise((resolve) => setTimeout(() => { written.push(chunk); resolve(); }, 10));
            },
            close() { written.push('closed'); },
        }, { highWaterMark: 2 });
        const writer = stream.getWriter();
        out.desired = writer.desiredSize;
        writer.write('a');
        writer.write('b');
        out.backpressure = writer.desiredSize;
        writer.close().then(() => {
            out.written = written;
            out.locked = stream.locked;
        });
        writer.closed.then(() => { out.closed = true; });
    "#)
    .await;
    assert_eq!(
        out,
        r#"{"desired":2,"backpressure":0,"written":["a","b","closed"],"locked":true,"closed":true}"#
    );
}

#[tokio::test]
async fn aborting_and_erroring_a_writable_stream() {
    let out = run(r#"
        const aborted = new WritableStream({ abort(reason) { out.abortReason = reason; } });
        aborted.abort('no').then(() => {
            const w = aborted.getWriter();
            w.write('x').catch((e) => { out.writeAfterAbort = e; });
        });

        let controller;
        const failing = new WritableStream({ start(c) { controller = c; } });
        const writer = failing.getWriter();
        controller.error(new Error('sink broke'));
        writer.closed.catch((e) => { out.closedError = e.message; });
        out.signal = controller.signal.aborted;

        const rejecting = new WritableStream({ write() { throw new Error('write failed'); } });
        rejecting.getWriter().write(1).catch((e) => { out.writeError = e.message; });
    "#)
    .await;
    assert_eq!(
        out,
        r#"{"signal":false,"abortReason":"no","closedError":"sink broke","writeError":"write failed","writeAfterAbort":"no"}"#
    );
}

#[tokio::test]
async fn pipe_through_a_transform_stream() {
    let out = run(r#"
        const upper = new TransformStream({
            transform(chunk, controller) { controller.enqueue(chunk.toUpperCase()); },
            flush(controller) { controller.enqueue('!'); },
        });
        const source = ReadableStream.from(['a', 'b']);
        const collected = [];
        source.pipeThrough(upper)
            .pipeTo(new WritableStream({ write(chunk) { collected.push(chunk); } }))
            .then(() => { out.collected = collected; out.sourceLocked = source.locked; });

        // With no transform, chunks pass through unchanged.
        const identity = new TransformStream();
        const writer = identity.writable.getWriter();
        writer.write(1);
        writer.close();
        identity.readable.getReader().read().then((r) => { out.identity = r.value; });
    "#)
    .await;
    assert_eq!(
        out,
        r#"{"identity":1,"collected":["A","B","!"],"sourceLocked":false}"#
    );
}

#[tokio::test]
async fn pipe_to_propagates_errors_and_abort_signals() {
    let out = run(r#"
        let controller;
        const source = new ReadableStream({ start(c) { controller = c; } });
        const dest = new WritableStream({ abort(reason) { out.destAborted = reason.message; } });
        source.pipeTo(dest).catch((e) => { out.pipeError = e.message; });
        controller.error(new Error('source failed'));

        const ac = new AbortController();
        const endless = new ReadableStream({
            pull(c) { c.enqueue(1); return new Promise((r) => setTimeout(r, 5)); },
            cancel() { out.sourceCancelled = true; },
        });
        endless.pipeTo(new WritableStream(), { signal: ac.signal })
            .catch((e) => { out.abortError = e.name; });
        setTimeout(() => ac.abort(), 20);
    "#)
    .await;
    assert_eq!(
        out,
        r#"{"destAborted":"source failed","pipeError":"source failed","sourceCancelled":true,"abortError":"AbortError"}"#
    );
}

#[tokio::test]
async fn text_encoder_and_decoder_streams() {
    let out = run(r#"
        const bytes = new TextEncoder().encode('héllo wörld');
        // Split inside the two-byte 'é' to check the decoder carries it over.
        const source = ReadableStream.from([bytes.slice(0, 2), bytes.slice(2)]);
        (async () => {
            let text = '';
            for await (const chunk of source.pipeThrough(new TextDecoderStream())) text += chunk;
            out.text = text;
            const encoded = ReadableStream.from(['ab', 'c']).pipeThrough(new TextEncoderStream());
            const lengths = [];
            for await (const chunk of encoded) lengths.push(chunk.constructor.name + ':' + chunk.length);
            out.encoded = lengths;
        })();
    "#)
    .await;
    assert_eq!(
        out,
        r#"{"text":"héllo wörld","encoded":["Uint8Array:2","Uint8Array:1"]}"#
    );
}

#[tokio::test]
async fn response_bodies_are_readable_streams() {
    let out = run(r#"
        const body = new Response('streamed').body;
        out.isStream = body instanceof ReadableStream;
        (async () => {
            out.text = await new Response(ReadableStream.from([
                new TextEncoder().encode('from '),
                new TextEncoder().encode('a stream'),
            ])).text();
            const chunks = [];
            for await (const c of body) chunks.push(new TextDecoder().decode(c));
            out.chunks = chunks;
            out.blob = await new Response(new Blob(['blob']).stream()).text();
            const used = ReadableStream.from([]);
            used.getReader();
            try { new Response(used); } catch (e) { out.locked = e.name; }
        })();
    "#)
    .await;
    assert_eq!(
        out,
        r#"{"isStream":true,"text":"from a stream","chunks":["streamed"],"blob":"blob","locked":"TypeError"}"#
    );
}

#[tokio::test]
async fn the_next_js_flight_data_pattern_works() {
    // Next.js's App Router feeds inline `self.__next_f` chunks to React
    // through a stream it writes from outside.
    let out = run(r#"
        const encoder = new TextEncoder();
        let writer;
        const readable = new ReadableStream({ start(c) { writer = c; } });
        writer.enqueue(encoder.encode('1:"a"\n'));
        setTimeout(() => {
            writer.enqueue(encoder.encode('2:"b"\n'));
            writer.close();
        }, 10);
        (async () => {
            const reader = readable.getReader();
            const decoder = new TextDecoder();
            let text = '';
            for (;;) {
                const { value, done } = await reader.read();
                if (done) break;
                text += decoder.decode(value, { stream: true });
            }
            out.text = text;
        })();
    "#)
    .await;
    assert_eq!(out, r#"{"text":"1:\"a\"\n2:\"b\"\n"}"#);
}
