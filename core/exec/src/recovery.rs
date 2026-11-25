//! Linux/Android process identity and adopted-child cleanup.
use std::path::{Path, PathBuf};

#[cfg(any(target_os = "linux", target_os = "android"))]
pub fn enable_subreaper() -> std::io::Result<()> {
    if unsafe { libc::prctl(libc::PR_SET_CHILD_SUBREAPER, 1, 0, 0, 0) } == -1 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(not(any(target_os = "linux", target_os = "android")))]
pub fn enable_subreaper() -> std::io::Result<()> {
    Ok(())
}

fn stat_fields(stat: &str) -> Option<(u32, u64)> {
    // comm can contain spaces and parentheses. The remainder starts at field 3.
    let fields: Vec<_> = stat.rsplit_once(") ")?.1.split_whitespace().collect();
    Some((fields.get(1)?.parse().ok()?, fields.get(19)?.parse().ok()?))
}

fn process(pid: u32) -> Option<(u32, u64)> {
    stat_fields(&std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?)
}

pub(crate) fn identity(pid: u32) -> Option<String> {
    let (_, start) = process(pid)?;
    let boot = std::fs::read_to_string("/proc/sys/kernel/random/boot_id").ok()?;
    Some(format!("{pid} {start} {}\n", boot.trim()))
}

pub(crate) fn verified_pid(record: &str) -> Option<u32> {
    let pid = record.split_whitespace().next()?.parse().ok()?;
    (identity(pid)?.trim() == record.trim()).then_some(pid)
}

/// Snapshot direct children before a serialized run; pre-existing app children
/// must never be swept. Start time also guards PID reuse within the snapshot.
pub(crate) fn children() -> Vec<(u32, u64)> {
    let mut children = Vec::new();
    if let Ok(entries) = std::fs::read_dir("/proc") {
        for entry in entries.flatten() {
            if let Some(pid) = entry.file_name().to_str().and_then(|s| s.parse().ok())
                && let Some((parent, start)) = process(pid)
                && parent == std::process::id()
            {
                children.push((pid, start));
            }
        }
    }
    children
}

pub(crate) fn sweep_children(before: &[(u32, u64)], leader: u32, began: u64) {
    #[cfg(any(target_os = "linux", target_os = "android"))]
    for _ in 0..20 {
        let mut found = false;
        for (pid, start) in children() {
            if pid == leader || start < began || before.contains(&(pid, start)) {
                continue;
            }
            // Check both parent and start time again immediately before signalling.
            if process(pid) != Some((std::process::id(), start)) {
                continue;
            }
            found = true;
            unsafe {
                libc::kill(pid as i32, libc::SIGKILL);
                libc::waitpid(pid as i32, std::ptr::null_mut(), libc::WNOHANG);
            }
        }
        if !found {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    #[cfg(not(any(target_os = "linux", target_os = "android")))]
    let _ = (before, leader, began);
}

pub(crate) fn start_tick() -> u64 {
    // /proc/uptime shares the process start-time clock, in clock ticks.
    #[cfg(any(target_os = "linux", target_os = "android"))]
    {
        let uptime = std::fs::read_to_string("/proc/uptime").unwrap_or_default();
        let seconds: f64 = uptime
            .split_whitespace()
            .next()
            .unwrap_or("0")
            .parse()
            .unwrap_or(0.0);
        (seconds * unsafe { libc::sysconf(libc::_SC_CLK_TCK) } as f64) as u64
    }
    #[cfg(not(any(target_os = "linux", target_os = "android")))]
    {
        0
    }
}

/// Discover only executor pidfiles, under the app-private spill tree.
pub fn sweep_spill(root: &Path) -> usize {
    fn visit(root: &Path, files: &mut Vec<PathBuf>) {
        if let Ok(entries) = std::fs::read_dir(root) {
            for entry in entries.flatten() {
                let Ok(kind) = entry.file_type() else {
                    continue;
                };
                if kind.is_dir() {
                    visit(&entry.path(), files);
                } else if kind.is_file()
                    && entry.file_name().to_string_lossy().starts_with("bash-")
                    && entry.path().extension().is_some_and(|e| e == "pid")
                {
                    files.push(entry.path());
                }
            }
        }
    }
    let mut files = Vec::new();
    visit(root, &mut files);
    crate::sweep_orphans(&files)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parses_start_time_after_parenthesized_comm() {
        let stat = format!(
            "123 (name ) with spaces) S 42 {} 987 0",
            vec!["0"; 17].join(" ")
        );
        assert_eq!(stat_fields(&stat), Some((42, 987)));
    }
    #[cfg(any(target_os = "linux", target_os = "android"))]
    #[test]
    fn identity_requires_pid_start_and_boot() {
        let record = identity(std::process::id()).unwrap();
        assert_eq!(verified_pid(&record), Some(std::process::id()));
        let fields: Vec<_> = record.split_whitespace().collect();
        assert_eq!(
            verified_pid(&format!("{} 0 {}", fields[0], fields[2])),
            None
        );
        assert_eq!(
            verified_pid(&format!("{} {} wrong-boot", fields[0], fields[1])),
            None
        );
        assert_eq!(verified_pid(fields[0]), None);
    }
}
