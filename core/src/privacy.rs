//! Keeps the process's memory, where the recording and its key live, out of crash dumps.
//!
//! A crash dump is a file the system writes on its own, outside the application's control, so
//! it's exactly the kind of leftover Hindsight must not produce. Each call is per-process and
//! leaves nothing behind once Hindsight exits; none of them writes a setting anywhere.

/// What could be turned off, for the spike's report.
pub fn keep_memory_out_of_crash_dumps() -> Vec<&'static str> {
    let mut done = Vec::new();
    #[cfg(unix)]
    unsafe {
        // A core dump is a copy of the process's memory; size zero means none is written.
        let none = libc::rlimit { rlim_cur: 0, rlim_max: 0 };
        if libc::setrlimit(libc::RLIMIT_CORE, &none) == 0 {
            done.push("core dumps off");
        }
    }
    #[cfg(target_os = "linux")]
    unsafe {
        // Also stops other processes of the same user from attaching and reading memory.
        if libc::prctl(libc::PR_SET_DUMPABLE, 0, 0, 0, 0) == 0 {
            done.push("not dumpable or traceable");
        }
    }
    #[cfg(windows)]
    unsafe {
        use windows_sys::Win32::System::ErrorReporting::{WER_FAULT_REPORTING_FLAG_NOHEAP, WerSetFlags};
        // Leaves heap memory out of Windows Error Reporting. Unlike excluding the application,
        // which writes a registry entry that would outlive Hindsight, this lasts only for the
        // process.
        if WerSetFlags(WER_FAULT_REPORTING_FLAG_NOHEAP) == 0 {
            done.push("heap left out of error reports");
        }
    }
    done
}
