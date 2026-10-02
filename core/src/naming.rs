//! Clip file names: when the clip ends, in local time, in a form every system accepts and that
//! sorts by date. "Hindsight 2026-10-01 15.07.12.opus"; colons aren't allowed on Windows.

use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LocalTime {
    pub year: i32,
    pub month: u32,
    pub day: u32,
    pub hour: u32,
    pub minute: u32,
    pub second: u32,
}

impl LocalTime {
    #[cfg(unix)]
    pub fn now() -> LocalTime {
        unsafe {
            let now = libc::time(std::ptr::null_mut());
            let mut parts: libc::tm = std::mem::zeroed();
            libc::localtime_r(&now, &mut parts);
            LocalTime {
                year: parts.tm_year + 1900,
                month: (parts.tm_mon + 1) as u32,
                day: parts.tm_mday as u32,
                hour: parts.tm_hour as u32,
                minute: parts.tm_min as u32,
                second: parts.tm_sec as u32,
            }
        }
    }

    #[cfg(windows)]
    pub fn now() -> LocalTime {
        use windows_sys::Win32::System::SystemInformation::GetLocalTime;
        let mut parts = unsafe { std::mem::zeroed() };
        unsafe { GetLocalTime(&mut parts) };
        LocalTime {
            year: i32::from(parts.wYear),
            month: u32::from(parts.wMonth),
            day: u32::from(parts.wDay),
            hour: u32::from(parts.wHour),
            minute: u32::from(parts.wMinute),
            second: u32::from(parts.wSecond),
        }
    }
}

impl LocalTime {
    /// The local time `ago` before now: when a clip that ends in the past ended.
    #[cfg(unix)]
    pub fn before_now(ago: std::time::Duration) -> LocalTime {
        unsafe {
            let then = libc::time(std::ptr::null_mut()) - ago.as_secs() as libc::time_t;
            let mut parts: libc::tm = std::mem::zeroed();
            libc::localtime_r(&then, &mut parts);
            LocalTime {
                year: parts.tm_year + 1900,
                month: (parts.tm_mon + 1) as u32,
                day: parts.tm_mday as u32,
                hour: parts.tm_hour as u32,
                minute: parts.tm_min as u32,
                second: parts.tm_sec as u32,
            }
        }
    }

    #[cfg(windows)]
    pub fn before_now(ago: std::time::Duration) -> LocalTime {
        use windows_sys::Win32::Foundation::{FILETIME, SYSTEMTIME};
        use windows_sys::Win32::System::SystemInformation::GetSystemTimeAsFileTime;
        use windows_sys::Win32::System::Time::{FileTimeToSystemTime, SystemTimeToTzSpecificLocalTime};
        unsafe {
            let mut now: FILETIME = std::mem::zeroed();
            GetSystemTimeAsFileTime(&mut now);
            // File times count 100-nanosecond ticks.
            let ticks = ((u64::from(now.dwHighDateTime) << 32) | u64::from(now.dwLowDateTime))
                .saturating_sub((ago.as_nanos() / 100) as u64);
            let then = FILETIME { dwLowDateTime: ticks as u32, dwHighDateTime: (ticks >> 32) as u32 };
            let mut universal: SYSTEMTIME = std::mem::zeroed();
            let mut local: SYSTEMTIME = std::mem::zeroed();
            FileTimeToSystemTime(&then, &mut universal);
            SystemTimeToTzSpecificLocalTime(std::ptr::null(), &universal, &mut local);
            LocalTime {
                year: i32::from(local.wYear),
                month: u32::from(local.wMonth),
                day: u32::from(local.wDay),
                hour: u32::from(local.wHour),
                minute: u32::from(local.wMinute),
                second: u32::from(local.wSecond),
            }
        }
    }
}

/// "Hindsight 2026-10-01 15.07.12"
pub fn clip_stem(time: LocalTime) -> String {
    format!(
        "Hindsight {:04}-{:02}-{:02} {:02}.{:02}.{:02}",
        time.year, time.month, time.day, time.hour, time.minute, time.second
    )
}

/// A path in `folder` for a clip ending at `time` that doesn't exist yet: two saves in the
/// same second get " (2)", " (3)" and so on, so a save never replaces another.
pub fn clip_path(folder: &Path, time: LocalTime) -> PathBuf {
    let stem = clip_stem(time);
    let mut path = folder.join(format!("{stem}.opus"));
    let mut copy = 2;
    while path.exists() {
        path = folder.join(format!("{stem} ({copy}).opus"));
        copy += 1;
    }
    path
}

#[cfg(test)]
mod tests {
    use super::*;

    const TIME: LocalTime = LocalTime { year: 2026, month: 10, day: 1, hour: 9, minute: 5, second: 3 };

    #[test]
    fn names_sort_by_date_and_avoid_colons() {
        assert_eq!(clip_stem(TIME), "Hindsight 2026-10-01 09.05.03");
        assert!(!clip_stem(TIME).contains(':'));
    }

    #[test]
    fn a_second_save_in_the_same_second_gets_a_number() {
        let folder = std::env::temp_dir().join(format!("hindsight-naming-{}", std::process::id()));
        std::fs::create_dir_all(&folder).unwrap();
        let first = clip_path(&folder, TIME);
        assert_eq!(first.file_name().unwrap(), "Hindsight 2026-10-01 09.05.03.opus");
        std::fs::write(&first, b"").unwrap();
        let second = clip_path(&folder, TIME);
        assert_eq!(second.file_name().unwrap(), "Hindsight 2026-10-01 09.05.03 (2).opus");
        std::fs::write(&second, b"").unwrap();
        assert_eq!(clip_path(&folder, TIME).file_name().unwrap(), "Hindsight 2026-10-01 09.05.03 (3).opus");
        std::fs::remove_dir_all(folder).unwrap();
    }

    #[test]
    fn a_time_in_the_past_is_that_long_ago() {
        let seconds_of_day = |time: LocalTime| i64::from(time.hour * 3600 + time.minute * 60 + time.second);
        let now = seconds_of_day(LocalTime::now());
        let earlier = seconds_of_day(LocalTime::before_now(std::time::Duration::from_secs(150)));
        let difference = (now - earlier).rem_euclid(86_400);
        // A second may tick over between the two readings.
        assert!((150..=151).contains(&difference), "{difference} s apart");
    }

    #[test]
    fn the_clock_reads_a_plausible_time() {
        let now = LocalTime::now();
        assert!(now.year >= 2026 && (1..=12).contains(&now.month) && (1..=31).contains(&now.day));
        assert!(now.hour < 24 && now.minute < 60 && now.second < 61);
    }
}
