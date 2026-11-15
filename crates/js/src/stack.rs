//! How much native stack the current thread has left, so the JS stack
//! limit can use it.
//!
//! QuickJS recurses on the native stack for every JS call (about 1.2 KiB
//! per call), so a fixed 1 MiB limit allowed only about 870 nested calls,
//! where Chrome allows about ten thousand. React's commit phase recurses
//! once per level of the component tree, and deep trees (open.spotify.com)
//! overflowed it. The limit is now as much as the thread can give, less a
//! margin for the host's own frames, up to
//! [`DEFAULT_MAX_STACK_SIZE`](crate::runtime::DEFAULT_MAX_STACK_SIZE).

/// Room left for Rust and C frames below the deepest JS frame (natives
/// called from JS, the allocator, the regexp matcher).
pub(crate) const MARGIN: usize = 256 * 1024;

/// Bytes between the current stack pointer and the end of this thread's
/// stack, if the platform says.
pub(crate) fn headroom() -> Option<usize> {
    let here = {
        let marker = 0u8;
        std::ptr::addr_of!(marker) as usize
    };
    let low = stack_low()?;
    here.checked_sub(low)
}

/// The lowest address of the current thread's stack.
#[cfg(target_os = "linux")]
fn stack_low() -> Option<usize> {
    // SAFETY: getpid and gettid have no preconditions.
    let main = unsafe { libc::getpid() == libc::gettid() };
    if main {
        // The main thread's stack grows on demand up to RLIMIT_STACK.
        // musl's pthread_getattr_np reports only what is mapped so far, so
        // use the limit, measured from the top of the stack (the
        // environment and auxiliary vector sit just below it).
        let mut limit = libc::rlimit {
            rlim_cur: 0,
            rlim_max: 0,
        };
        // SAFETY: `limit` is a valid rlimit to write to.
        if unsafe { libc::getrlimit(libc::RLIMIT_STACK, &mut limit) } != 0 {
            return None;
        }
        let size = if limit.rlim_cur == libc::RLIM_INFINITY {
            8 * 1024 * 1024
        } else {
            usize::try_from(limit.rlim_cur).ok()?
        };
        let top = main_stack_top()?;
        return top.checked_sub(size);
    }
    // SAFETY: pthread_getattr_np fills `attr`, which is destroyed after.
    unsafe {
        let mut attr = std::mem::zeroed::<libc::pthread_attr_t>();
        if libc::pthread_getattr_np(libc::pthread_self(), &mut attr) != 0 {
            return None;
        }
        let mut addr = std::ptr::null_mut();
        let mut size = 0;
        let ok = libc::pthread_attr_getstack(&attr, &mut addr, &mut size) == 0;
        libc::pthread_attr_destroy(&mut attr);
        // The guard page sits at the low end.
        ok.then(|| addr as usize + 4096)
    }
}

/// The top of the main thread's stack: the end of the `[stack]` mapping.
#[cfg(target_os = "linux")]
fn main_stack_top() -> Option<usize> {
    let maps = std::fs::read_to_string("/proc/self/maps").ok()?;
    let line = maps.lines().find(|l| l.ends_with("[stack]"))?;
    let range = line.split_whitespace().next()?;
    let (_, end) = range.split_once('-')?;
    usize::from_str_radix(end, 16).ok()
}

/// The lowest address of the current thread's stack.
#[cfg(target_os = "macos")]
fn stack_low() -> Option<usize> {
    // SAFETY: these read the calling thread's own record and the page
    // size, and cannot fail.
    unsafe {
        let thread = libc::pthread_self();
        // The address is the high end of the stack, and the size may
        // count the guard page at the low end.
        let top = libc::pthread_get_stackaddr_np(thread) as usize;
        let page = usize::try_from(libc::sysconf(libc::_SC_PAGESIZE)).ok()?;
        top.checked_sub(libc::pthread_get_stacksize_np(thread))?
            .checked_add(page)
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn stack_low() -> Option<usize> {
    None
}

/// The JS stack limit to use for `wanted` bytes on this thread.
pub(crate) fn limit(wanted: usize) -> usize {
    match headroom() {
        Some(room) => wanted.min(room.saturating_sub(MARGIN)).max(64 * 1024),
        // Unknown platform: what was asked for, capped at what a default
        // Rust thread (2 MiB) can take.
        None => wanted.min(2 * 1024 * 1024 - MARGIN),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    fn a_spawned_thread_reports_its_stack() {
        let room = std::thread::Builder::new()
            .stack_size(4 * 1024 * 1024)
            .spawn(headroom)
            .unwrap()
            .join()
            .unwrap()
            .expect("known on Linux and macOS");
        assert!((3 * 1024 * 1024..4 * 1024 * 1024).contains(&room), "{room}");
        let small = std::thread::Builder::new()
            .stack_size(512 * 1024)
            .spawn(|| limit(8 * 1024 * 1024))
            .unwrap()
            .join()
            .unwrap();
        // glibc may reuse a cached stack of up to four times the size.
        assert!(small < 2 * 1024 * 1024 - MARGIN, "{small}");
    }
}
