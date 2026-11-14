//! The event loop: the task queue, timers and animation frames, and the
//! host-side methods that drive them.
//!
//! The loop is a simplified version of the HTML event loop. Each turn takes
//! one task (a due timer or a host task, in the order they became runnable)
//! and runs it, then performs a microtask checkpoint. Animation frame
//! callbacks run in a "rendering" step whenever the virtual frame clock
//! reaches the next frame boundary and callbacks are waiting.

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use rquickjs::{Ctx, Function, JsLifetime, Object, Persistent};

use crate::budget::Budget;
use crate::clock::{Clock, ClockMode};
use crate::limits::ResourceLimit;
use crate::remote::TaskSender;
use crate::runtime::{JsData, JsRuntime, Shared};
use crate::timers::{TimerId, Timers};

/// Default spacing of animation frames: 60 per second.
pub const DEFAULT_FRAME_INTERVAL: Duration = Duration::from_nanos(16_666_667);

/// A unit of host work run on the event loop with access to the JS context.
///
/// Errors it returns are reported like an exception escaping any other
/// task: fired at the global as an `error` event, then logged unless a
/// listener canceled it.
pub type Task = Box<dyn for<'js> FnOnce(Ctx<'js>) -> rquickjs::Result<()>>;

/// Why the event loop stopped before running out of work.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Stopped {
    /// The runtime's deadline passed.
    DeadlineExceeded,
    /// An [`InterruptHandle`](crate::InterruptHandle) asked it to stop.
    Interrupted,
    /// The page went over a resource limit, and is stopped for good.
    LimitExceeded(ResourceLimit),
    /// The page used up its CPU time or request budget (see
    /// [`JsRuntime::set_work_limits`]).
    BudgetExhausted(Budget),
}

impl std::fmt::Display for Stopped {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::DeadlineExceeded => f.write_str("the JS deadline passed"),
            Self::Interrupted => f.write_str("JS execution was interrupted"),
            Self::LimitExceeded(limit) => limit.fmt(f),
            Self::BudgetExhausted(budget) => budget.fmt(f),
        }
    }
}

impl std::error::Error for Stopped {}

enum Queued {
    Timer(TimerId),
    Host(Task),
}

/// Animation frame requests waiting for the next frame.
#[derive(Debug, Default)]
struct Frames {
    last_id: u32,
    /// Ids of the callbacks to run in the next frame, in request order.
    pending: Vec<u32>,
    /// Page time at which `pending` became non-empty.
    requested_at: Option<Duration>,
}

pub(crate) struct LoopState {
    /// Shared with the DOM, which timestamps mutations.
    pub(crate) clock: Rc<Clock>,
    frame_interval: Duration,
    timers: RefCell<Timers>,
    tasks: RefCell<VecDeque<Queued>>,
    frames: RefCell<Frames>,
    /// Timer nesting level of the running task; 0 when it is not a timer.
    timer_nesting: Cell<u32>,
    /// A scheduled declarative refresh: when it is due and where it goes.
    pub(crate) refresh: RefCell<Option<(Duration, String)>>,
    /// When [`JsRuntime::run`] last gave the async executor its turn.
    pub(crate) yielded_at: Cell<Instant>,
}

impl LoopState {
    pub(crate) fn new(mode: ClockMode, frame_interval: Duration) -> Self {
        Self {
            clock: Rc::new(Clock::new(mode)),
            frame_interval: frame_interval.max(Duration::from_micros(1)),
            timers: RefCell::default(),
            tasks: RefCell::default(),
            frames: RefCell::default(),
            timer_nesting: Cell::new(0),
            refresh: RefCell::new(None),
            yielded_at: Cell::new(Instant::now()),
        }
    }

    /// The first frame boundary strictly after `time`.
    fn frame_after(&self, time: Duration) -> Duration {
        let interval = self.frame_interval.as_nanos();
        let n = time.as_nanos() / interval + 1;
        nanos(n * interval)
    }

    /// The last frame boundary at or before `time`.
    fn frame_at(&self, time: Duration) -> Duration {
        let interval = self.frame_interval.as_nanos();
        nanos(time.as_nanos() / interval * interval)
    }

    fn next_frame(&self) -> Option<Duration> {
        self.frames
            .borrow()
            .requested_at
            .map(|requested| self.frame_after(requested))
    }

    /// When the next timer, animation frame or refresh is due, in page
    /// time.
    fn next_wakeup(&self) -> Option<Duration> {
        let timer = self.timers.borrow().next_due();
        self.next_of(timer)
    }

    /// When the next thing that holds the page open is due: as
    /// [`next_wakeup`](Self::next_wakeup), leaving out idle callbacks.
    /// Those run when they are due, but a page with nothing else to do
    /// is idle.
    pub(crate) fn next_holding_wakeup(&self) -> Option<Duration> {
        let timer = self.timers.borrow().next_holding_due();
        self.next_of(timer)
    }

    fn next_of(&self, timer: Option<Duration>) -> Option<Duration> {
        let refresh = self.refresh.borrow().as_ref().map(|&(due, _)| due);
        [timer, self.next_frame(), refresh]
            .into_iter()
            .flatten()
            .min()
    }

    /// Takes the scheduled refresh off the schedule if it is due at `now`.
    fn pop_due_refresh(&self, now: Duration) -> Option<String> {
        let mut refresh = self.refresh.borrow_mut();
        if refresh.as_ref().is_some_and(|&(due, _)| due <= now) {
            refresh.take().map(|(_, url)| url)
        } else {
            None
        }
    }

    fn set_timer(&self, timeout_ms: u32, repeat: bool) -> TimerId {
        let timeout = Duration::from_millis(u64::from(timeout_ms));
        let now = self.clock.now();
        let nesting = self.timer_nesting.get();
        self.timers
            .borrow_mut()
            .insert(now, timeout, repeat, nesting)
    }

    fn set_idle_timer(&self, timeout_ms: u32) -> TimerId {
        let timeout = Duration::from_millis(u64::from(timeout_ms));
        let now = self.clock.now();
        let nesting = self.timer_nesting.get();
        self.timers.borrow_mut().insert_idle(now, timeout, nesting)
    }

    fn request_frame(&self) -> u32 {
        let mut frames = self.frames.borrow_mut();
        frames.last_id = frames.last_id.wrapping_add(1).max(1);
        let id = frames.last_id;
        if frames.pending.is_empty() {
            frames.requested_at = Some(self.clock.now());
        }
        frames.pending.push(id);
        id
    }

    fn cancel_frame(&self, id: u32) {
        let mut frames = self.frames.borrow_mut();
        frames.pending.retain(|&p| p != id);
        if frames.pending.is_empty() {
            frames.requested_at = None;
        }
    }

    pub(crate) fn queue(&self, task: Task) {
        self.tasks.borrow_mut().push_back(Queued::Host(task));
    }

    /// Drops queued work. Host tasks may hold JS values, which must be
    /// released while the runtime is still alive.
    pub(crate) fn clear(&self) {
        self.tasks.borrow_mut().clear();
    }
}

fn nanos(n: u128) -> Duration {
    Duration::from_nanos(u64::try_from(n).unwrap_or(u64::MAX))
}

fn ms(time: Duration) -> f64 {
    time.as_secs_f64() * 1000.0
}

/// Access to the event loop from host bindings running inside JS calls,
/// where the [`JsRuntime`] itself is not reachable.
#[derive(Clone)]
pub struct LoopHandle {
    shared: Rc<Shared>,
}

// SAFETY: `LoopHandle` holds no JS values and has no lifetime parameter, so
// `Changed` is the type itself.
unsafe impl<'js> JsLifetime<'js> for LoopHandle {
    type Changed<'to> = LoopHandle;
}

impl LoopHandle {
    /// The handle of the runtime that owns `ctx`.
    pub fn from_ctx(ctx: &Ctx<'_>) -> Option<Self> {
        ctx.userdata::<LoopHandle>().map(|h| h.clone())
    }

    /// Queues `task` to run as its own task on the event loop.
    pub fn queue_task(
        &self,
        task: impl for<'js> FnOnce(Ctx<'js>) -> rquickjs::Result<()> + 'static,
    ) {
        self.shared.event_loop.queue(Box::new(task));
    }

    /// Current page time since the time origin.
    pub fn now(&self) -> Duration {
        self.shared.event_loop.clock.now()
    }

    /// A sender for posting tasks from other threads.
    pub fn task_sender(&self) -> TaskSender {
        TaskSender::new(Arc::clone(&self.shared.remote))
    }
}

impl JsRuntime {
    /// Queues `task` to run as its own task on the event loop.
    pub fn queue_task(
        &self,
        task: impl for<'js> FnOnce(Ctx<'js>) -> rquickjs::Result<()> + 'static,
    ) {
        self.event_loop().queue(Box::new(task));
    }

    /// A handle for queueing tasks and reading page time.
    pub fn loop_handle(&self) -> LoopHandle {
        LoopHandle {
            shared: Rc::clone(self.shared()),
        }
    }

    /// Current page time since the time origin.
    pub fn now(&self) -> Duration {
        self.event_loop().clock.now()
    }

    /// A sender for posting tasks to this runtime from other threads.
    pub fn task_sender(&self) -> TaskSender {
        TaskSender::new(Arc::clone(&self.shared().remote))
    }

    /// How page time relates to the wall clock.
    pub fn clock_mode(&self) -> ClockMode {
        self.event_loop().clock.mode()
    }

    /// The page time at which the next timer, animation frame or
    /// [refresh](Self::schedule_refresh) is due, if any are scheduled. May
    /// be in the past when work is already runnable.
    pub fn next_wakeup(&self) -> Option<Duration> {
        self.event_loop().next_wakeup()
    }

    /// Number of live `setTimeout` / `setInterval` timers.
    /// `requestIdleCallback` callbacks are not counted.
    pub fn pending_timers(&self) -> usize {
        self.event_loop().timers.borrow().holding()
    }

    /// Whether there is nothing left to run: no queued tasks, timers,
    /// animation frame callbacks or microtasks, and no host operations in
    /// progress. Callbacks waiting in `requestIdleCallback` do not count:
    /// they run when page time passes, but do not keep a page busy.
    pub fn is_idle(&self) -> bool {
        let state = self.event_loop();
        let remote = &self.shared().remote;
        !remote.has_pending()
            && !remote.has_queued()
            && state.tasks.borrow().is_empty()
            && state.timers.borrow().holding() == 0
            && state.frames.borrow().pending.is_empty()
            && state.refresh.borrow().is_none()
            && !self.has_pending_microtasks()
            && !self.shared().dom.layout_changed()
    }

    /// Whether work is runnable right now: a queued task, a task posted
    /// from another thread, a timer or animation frame that is due, or
    /// microtasks. `requestIdleCallback` callbacks do not count. A page
    /// in the middle of a run of tasks is not quiet, however long since
    /// its document last changed.
    pub fn has_work_due(&self) -> bool {
        let state = self.event_loop();
        let now = state.clock.now();
        !state.tasks.borrow().is_empty()
            || self.shared().remote.has_queued()
            || state.next_holding_wakeup().is_some_and(|due| due <= now)
            || self.has_pending_microtasks()
    }

    /// Runs everything that is runnable at the current page time (queued
    /// tasks, due timers and a due animation frame, each followed by a
    /// microtask checkpoint) until nothing is. Does not move the clock.
    ///
    /// Returns how many tasks and frames ran.
    pub fn run_ready(&self) -> Result<usize, Stopped> {
        let mut ran = 0;
        while self.step()? {
            ran += 1;
        }
        Ok(ran)
    }

    /// Fast-forwards page time by `by`, running each timer and animation
    /// frame as page time reaches it, in order. Works in every clock mode;
    /// in [`ClockMode::Realtime`] it is an explicit skip.
    ///
    /// Returns how many tasks and frames ran.
    pub fn advance(&self, by: Duration) -> Result<usize, Stopped> {
        let clock = &self.event_loop().clock;
        let target = clock.now() + by;
        let mut ran = self.run_ready()?;
        while let Some(next) = self.next_wakeup().filter(|&t| t <= target) {
            clock.advance_to(next);
            ran += self.run_ready()?;
        }
        clock.advance_to(target);
        Ok(ran + self.run_ready()?)
    }

    /// Runs one turn of the event loop if anything is runnable now. Returns
    /// whether something ran.
    pub(crate) fn step(&self) -> Result<bool, Stopped> {
        self.check_stop()?;
        let state = self.event_loop();
        let arrived = self.shared().remote.drain();
        state
            .tasks
            .borrow_mut()
            .extend(arrived.into_iter().map(|task| Queued::Host(task)));
        let now = state.clock.now();
        if let Some(url) = state.pop_due_refresh(now) {
            self.shared().window.request_refresh(url);
            return Ok(true);
        }
        {
            let mut timers = state.timers.borrow_mut();
            let mut tasks = state.tasks.borrow_mut();
            while let Some(id) = timers.pop_due(now) {
                tasks.push_back(Queued::Timer(id));
            }
        }
        if state.next_frame().is_some_and(|due| due <= now) {
            self.run_frame(state.frame_at(now));
        } else {
            let task = state.tasks.borrow_mut().pop_front();
            let Some(task) = task else {
                // The queue has drained: let layout observers catch up with
                // the tree, as a browser's rendering step would.
                if !self.update_layout() {
                    return Ok(false);
                }
                self.check_stop()?;
                return Ok(true);
            };
            match task {
                Queued::Timer(id) => self.run_timer(id),
                Queued::Host(task) => {
                    let _ = self.run_task(|ctx| task(ctx.clone()));
                }
            }
            self.perform_microtask_checkpoint();
        }
        self.check_stop()?;
        Ok(true)
    }

    fn check_stop(&self) -> Result<(), Stopped> {
        match self.shared().stop_reason() {
            Some(reason) => Err(reason),
            None => Ok(()),
        }
    }

    fn run_timer(&self, id: TimerId) {
        let state = self.event_loop();
        let Some(fire) = state.timers.borrow_mut().start(id) else {
            return;
        };
        state.timer_nesting.set(fire.nesting);
        let _ = self.run_task(|ctx| {
            let hook = JsData::hooks(ctx)?.run_timer;
            hook.call((id, fire.repeat))
        });
        state.timer_nesting.set(0);
        if fire.repeat {
            let now = state.clock.now();
            state.timers.borrow_mut().reschedule(id, now);
        }
    }

    /// The rendering step: runs every callback requested before this frame,
    /// each followed by a microtask checkpoint. Callbacks requested during
    /// the frame wait for the next one.
    fn run_frame(&self, frame_time: Duration) {
        let ids = {
            let mut frames = self.event_loop().frames.borrow_mut();
            frames.requested_at = None;
            std::mem::take(&mut frames.pending)
        };
        let timestamp = ms(frame_time);
        for id in ids {
            if self.shared().stop_reason().is_some() {
                break;
            }
            let _ = self.run_task(|ctx| {
                let hook = JsData::hooks(ctx)?.run_frame;
                hook.call((id, timestamp))
            });
            self.perform_microtask_checkpoint();
        }
    }

    pub(crate) fn event_loop(&self) -> &LoopState {
        &self.shared().event_loop
    }
}

/// JS hooks the loop calls back into.
#[derive(Clone)]
pub(crate) struct LoopHooks<'js> {
    pub(crate) run_timer: Function<'js>,
    pub(crate) run_frame: Function<'js>,
}

impl<'js> LoopHooks<'js> {
    pub(crate) fn from_prelude(hooks: &Object<'js>) -> rquickjs::Result<Self> {
        Ok(Self {
            run_timer: hooks.get("runTimer")?,
            run_frame: hooks.get("runFrame")?,
        })
    }
}

/// Adds the event loop's host functions to the prelude's `native` object.
pub(crate) fn install<'js>(
    ctx: &Ctx<'js>,
    native: &Object<'js>,
    shared: &Rc<Shared>,
) -> rquickjs::Result<()> {
    let s = Rc::clone(shared);
    native.set(
        "setTimer",
        Function::new(ctx.clone(), move |timeout: u32, repeat: bool| {
            s.event_loop.set_timer(timeout, repeat)
        })?,
    )?;
    let s = Rc::clone(shared);
    native.set(
        "setIdleTimer",
        Function::new(ctx.clone(), move |timeout: u32| {
            s.event_loop.set_idle_timer(timeout)
        })?,
    )?;
    let s = Rc::clone(shared);
    native.set(
        "clearTimer",
        Function::new(ctx.clone(), move |id: u32| {
            s.event_loop.timers.borrow_mut().remove(id)
        })?,
    )?;
    let s = Rc::clone(shared);
    native.set(
        "requestFrame",
        Function::new(ctx.clone(), move || s.event_loop.request_frame())?,
    )?;
    let s = Rc::clone(shared);
    native.set(
        "cancelFrame",
        Function::new(ctx.clone(), move |id: u32| s.event_loop.cancel_frame(id))?,
    )?;
    let s = Rc::clone(shared);
    native.set(
        "queueTask",
        Function::new(
            ctx.clone(),
            move |ctx: Ctx<'js>, callback: Function<'js>| {
                let callback = Persistent::save(&ctx, callback);
                s.event_loop.queue(Box::new(move |ctx| {
                    callback.restore(&ctx)?.call::<_, ()>(())
                }));
            },
        )?,
    )?;
    let s = Rc::clone(shared);
    native.set(
        "now",
        Function::new(ctx.clone(), move || ms(s.event_loop.clock.now()))?,
    )?;
    native.set("timeOrigin", shared.event_loop.clock.epoch_ms())?;
    if ctx
        .store_userdata(LoopHandle {
            shared: Rc::clone(shared),
        })
        .is_err()
    {
        return Err(rquickjs::Error::Unknown);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_boundaries() {
        let state = LoopState::new(ClockMode::Manual, Duration::from_millis(10));
        let ms = Duration::from_millis;
        assert_eq!(state.frame_after(ms(0)), ms(10));
        assert_eq!(state.frame_after(ms(10)), ms(20));
        assert_eq!(state.frame_after(ms(15)), ms(20));
        assert_eq!(state.frame_at(ms(15)), ms(10));
        assert_eq!(state.frame_at(ms(20)), ms(20));
    }
}
