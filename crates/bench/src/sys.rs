//! The process's memory, as the operating system sees it.

/// The process's resident set size now, in bytes.
pub fn rss() -> u64 {
    imp::rss()
}

/// The largest resident set size the process has had, in bytes.
pub fn peak_rss() -> u64 {
    imp::peak_rss()
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

    #[cfg(any(target_os = "linux", target_vendor = "apple"))]
    #[test]
    fn reports_resident_memory() {
        let before = rss();
        assert!(before > 0);
        let block = vec![1u8; 32 << 20];
        assert!(rss() >= before + (16 << 20));
        assert!(peak_rss() >= rss());
        drop(block);
    }
}
