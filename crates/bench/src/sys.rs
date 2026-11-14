//! The process's memory, as the operating system sees it.

/// The process's resident set size now, in bytes.
pub fn rss() -> u64 {
    imp::rss()
}

/// The largest resident set size the process has had, in bytes.
pub fn peak_rss() -> u64 {
    imp::peak_rss()
}

/// CPU time the calling thread has used so far, user and system.
#[cfg(unix)]
pub fn thread_cpu() -> std::time::Duration {
    let mut ts = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: `ts` is a writable `timespec`; the clock exists on Linux
    // and on macOS 10.12 and later.
    let status = unsafe { libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, &mut ts) };
    if status != 0 {
        return std::time::Duration::ZERO;
    }
    let secs = u64::try_from(ts.tv_sec).unwrap_or(0);
    let nanos = u32::try_from(ts.tv_nsec).unwrap_or(0);
    std::time::Duration::new(secs, nanos)
}

#[cfg(not(unix))]
pub fn thread_cpu() -> std::time::Duration {
    std::time::Duration::ZERO
}

#[cfg(target_os = "linux")]
mod imp {
    /// A `kB` field of `/proc/self/status`, in bytes.
    fn status_field(name: &str) -> u64 {
        let status = std::fs::read_to_string("/proc/self/status").unwrap_or_default();
        status
            .lines()
            .find_map(|line| line.strip_prefix(name)?.strip_prefix(':'))
            .and_then(|rest| rest.trim().strip_suffix("kB")?.trim().parse::<u64>().ok())
            .map_or(0, |kb| kb * 1024)
    }

    pub fn rss() -> u64 {
        status_field("VmRSS")
    }

    pub fn peak_rss() -> u64 {
        status_field("VmHWM")
    }
}

#[cfg(target_vendor = "apple")]
mod imp {
    use std::mem::MaybeUninit;

    fn usage() -> Option<libc::rusage_info_v2> {
        let mut info = MaybeUninit::<libc::rusage_info_v2>::zeroed();
        // SAFETY: `info` is a writable `rusage_info_v2`, the struct the
        // `RUSAGE_INFO_V2` flavor fills in, for this process's own pid.
        let status = unsafe {
            libc::proc_pid_rusage(
                libc::getpid(),
                libc::RUSAGE_INFO_V2,
                info.as_mut_ptr().cast(),
            )
        };
        // SAFETY: on success the call initialized the struct; it started
        // zeroed in any case, and every field is an integer.
        (status == 0).then(|| unsafe { info.assume_init() })
    }

    pub fn rss() -> u64 {
        usage().map_or(0, |u| u.ri_resident_size)
    }

    pub fn peak_rss() -> u64 {
        let mut usage = MaybeUninit::<libc::rusage>::zeroed();
        // SAFETY: `usage` is a writable `rusage`.
        let status = unsafe { libc::getrusage(libc::RUSAGE_SELF, usage.as_mut_ptr()) };
        if status != 0 {
            return 0;
        }
        // SAFETY: initialized by the successful call. On macOS `ru_maxrss`
        // is in bytes.
        let max = unsafe { usage.assume_init() }.ru_maxrss;
        u64::try_from(max).unwrap_or(0)
    }
}

#[cfg(not(any(target_os = "linux", target_vendor = "apple")))]
mod imp {
    pub fn rss() -> u64 {
        0
    }

    pub fn peak_rss() -> u64 {
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn counts_this_threads_cpu_time() {
        let before = thread_cpu();
        let mut x = 0u64;
        while thread_cpu() < before + std::time::Duration::from_millis(5) {
            x = std::hint::black_box(x.wrapping_add(1));
        }
        assert!(x > 0);
    }

    #[cfg(any(target_os = "linux", target_vendor = "apple"))]
    #[test]
    fn reports_resident_memory() {
        let before = rss();
        assert!(before > 0);
        let block = vec![1u8; 32 << 20];
        let now = rss();
        assert!(now >= before + (16 << 20));
        // Read after: other tests load pages meanwhile, and the high-water
        // mark only rises.
        assert!(peak_rss() >= now);
        drop(block);
    }
}
