//! Integration tests for the Executor contract. Run on the dev host (MSYS2
//! sh) and for real on Linux CI -- both are "host" for these purposes; the
//! on-device pass happens via scripts/exec-matrix.ps1 + M3 device runs.

use std::time::{Duration, Instant};

#[cfg(any(target_os = "linux", target_os = "android"))]
use palmcode_exec::sweep_orphans;
use palmcode_exec::{Cancel, CaptureLimits, EnvSpec, Executor, HostExecutor};

fn env(dir: &std::path::Path) -> EnvSpec {
    EnvSpec {
        path: std::env::var("PATH").unwrap_or_default(),
        home: dir.to_path_buf(),
        tmpdir: dir.to_path_buf(),
        extra: Default::default(),
    }
}

fn exec() -> HostExecutor {
    HostExecutor::new()
}

#[test]
fn echo_exit_zero_and_output() {
    let dir = tempfile::tempdir().unwrap();
    let mut out = Vec::new();
    let res = exec()
        .run(
            &exec().spec("echo hi", dir.path(), env(dir.path())),
            &mut |b: &[u8]| out.extend_from_slice(b),
        )
        .unwrap();
    assert_eq!(res.exit, Some(0));
    assert!(!res.timed_out);
    assert_eq!(String::from_utf8_lossy(&out), "hi\n");
}

#[test]
fn exit_code_passes_through() {
    let dir = tempfile::tempdir().unwrap();
    let res = exec()
        .run(
            &exec().spec("exit 7", dir.path(), env(dir.path())),
            &mut |_b: &[u8]| {},
        )
        .unwrap();
    assert_eq!(res.exit, Some(7));
}

#[test]
fn stdout_and_stderr_merge() {
    let dir = tempfile::tempdir().unwrap();
    let mut out = Vec::new();
    exec()
        .run(
            &exec().spec("echo out; echo err 1>&2", dir.path(), env(dir.path())),
            &mut |b: &[u8]| out.extend_from_slice(b),
        )
        .unwrap();
    let s = String::from_utf8_lossy(&out);
    assert!(s.contains("out") && s.contains("err"), "merged: {s:?}");
}

#[test]
fn stdin_is_closed_immediate_eof() {
    let dir = tempfile::tempdir().unwrap();
    let started = Instant::now();
    let res = exec()
        .run(
            &exec().spec("cat", dir.path(), env(dir.path())),
            &mut |_b: &[u8]| {},
        )
        .unwrap();
    assert_eq!(res.exit, Some(0));
    assert!(started.elapsed() < Duration::from_secs(5));
}

#[test]
fn timeout_still_applies_after_both_pipes_close() {
    let dir = tempfile::tempdir().unwrap();
    let mut spec = exec().spec("exec >&- 2>&-; sleep 300", dir.path(), env(dir.path()));
    spec.timeout = Duration::from_millis(150);
    let started = Instant::now();
    let result = exec().run(&spec, &mut |_: &[u8]| {}).unwrap();
    assert!(result.timed_out);
    assert!(started.elapsed() < Duration::from_secs(5));
}

#[cfg(any(target_os = "linux", target_os = "android"))]
#[test]
fn successful_leader_exit_kills_closed_pipe_and_setsid_children() {
    for script in [
        "(exec >/dev/null 2>&1; sleep 0.5; echo survived > survivor) & sleep 0.1; exit 0",
        "setsid sh -c 'exec >/dev/null 2>&1; sleep 0.5; echo survived > survivor' & sleep 0.1; exit 0",
    ] {
        let dir = tempfile::tempdir().unwrap();
        exec()
            .run(
                &exec().spec(script, dir.path(), env(dir.path())),
                &mut |_: &[u8]| {},
            )
            .unwrap();
        std::thread::sleep(Duration::from_millis(700));
        assert!(!dir.path().join("survivor").exists(), "{script}");
    }
}

#[test]
fn output_cap_bounds_sink_capture_and_spill_and_drains_to_exit() {
    let dir = tempfile::tempdir().unwrap();
    let mut spec = exec().spec(
        "head -c 34603008 /dev/zero; echo done > completed",
        dir.path(),
        env(dir.path()),
    );
    spec.spill_dir = Some(dir.path().join("spill"));
    let mut streamed = 0;
    let result = exec()
        .run(&spec, &mut |bytes: &[u8]| {
            streamed += bytes.len();
        })
        .unwrap();
    assert_eq!(result.exit, Some(0));
    assert!(
        dir.path().join("completed").exists(),
        "over-cap output must be drained"
    );
    assert_eq!(streamed, palmcode_exec::OUTPUT_CAP);
    assert_eq!(
        result.captured.total_bytes,
        palmcode_exec::OUTPUT_CAP as u64
    );
    assert!(
        result
            .captured
            .render()
            .contains("remaining output discarded")
    );
    assert_eq!(
        std::fs::metadata(result.spill.unwrap()).unwrap().len(),
        palmcode_exec::OUTPUT_CAP as u64
    );
}

#[cfg(any(target_os = "linux", target_os = "android"))]
#[test]
fn sweep_rejects_reused_pid_and_wrong_boot_and_consumes_legacy_records() {
    let dir = tempfile::tempdir().unwrap();
    let pid = std::process::id();
    let boot = std::fs::read_to_string("/proc/sys/kernel/random/boot_id").unwrap();
    for record in [
        format!("{pid} 0 {}", boot.trim()),
        format!("{pid} 0 wrong-boot"),
        pid.to_string(),
    ] {
        let file = dir.path().join("bash-stale.pid");
        std::fs::write(&file, record).unwrap();
        assert_eq!(palmcode_exec::sweep_spill(dir.path()), 0);
        assert!(!file.exists());
    }
}

#[test]
fn timeout_kills_tree_including_grandchildren() {
    let dir = tempfile::tempdir().unwrap();
    // Counter lives in the cwd: no host path quoting inside the script.
    let counter = dir.path().join("counter.txt");
    // The counting loop is a subshell grandchild of `sh`; if the kill only
    // hit the leader it would keep writing after the timeout.
    let script = "while true; do echo x >> counter.txt; sleep 0.2; done & wait";
    let mut spec = exec().spec(script, dir.path(), env(dir.path()));
    spec.timeout = Duration::from_millis(700);
    let started = Instant::now();
    let res = exec().run(&spec, &mut |_b: &[u8]| {}).unwrap();
    assert!(res.timed_out);
    assert!(started.elapsed() < Duration::from_secs(10));
    let size_at_kill = std::fs::metadata(&counter).map(|m| m.len()).unwrap_or(0);
    std::thread::sleep(Duration::from_millis(700));
    let size_later = std::fs::metadata(&counter).map(|m| m.len()).unwrap_or(0);
    assert_eq!(
        size_at_kill, size_later,
        "grandchild survived the tree kill"
    );
}

#[test]
fn cancel_stops_the_run() {
    let dir = tempfile::tempdir().unwrap();
    let cancel = Cancel::default();
    let mut spec = exec().spec("sleep 30", dir.path(), env(dir.path()));
    spec.timeout = Duration::from_secs(60);
    spec.cancel = cancel.clone();
    let handle = cancel.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(200));
        handle.cancel();
    });
    let started = Instant::now();
    let res = exec().run(&spec, &mut |_b: &[u8]| {}).unwrap();
    assert!(res.cancelled);
    assert!(started.elapsed() < Duration::from_secs(10));
}

#[test]
fn head_tail_capture_and_spill() {
    let dir = tempfile::tempdir().unwrap();
    let mut spec = exec().spec(
        "i=0; while [ $i -lt 5000 ]; do echo line-$i; i=$((i+1)); done",
        dir.path(),
        env(dir.path()),
    );
    spec.capture = CaptureLimits { head: 64, tail: 64 };
    spec.spill_dir = Some(dir.path().join("spill"));
    let res = exec().run(&spec, &mut |_b: &[u8]| {}).unwrap();
    let rendered = res.captured.render();
    assert!(rendered.starts_with("line-0"), "head: {rendered:.40}");
    assert!(rendered.ends_with("line-4999\n"), "tail: ...{rendered:.40}");
    assert!(rendered.contains("bytes elided"));
    assert_eq!(res.captured.total_lines, 5000);
    let spill = res.spill.expect("spill file");
    let full = std::fs::read_to_string(&spill).unwrap();
    assert_eq!(full.lines().count(), 5000);
}

#[test]
fn env_is_not_inherited_from_the_parent() {
    let dir = tempfile::tempdir().unwrap();
    // A secret in OUR env must never surface in the child's.
    // (set_var is unsafe in edition 2024; scope the mutation tightly.)
    unsafe { std::env::set_var("PALM_TEST_SECRET", "sk-do-not-leak") };
    let res = exec()
        .run(
            &exec().spec("env", dir.path(), env(dir.path())),
            &mut |_b: &[u8]| {},
        )
        .unwrap();
    unsafe { std::env::remove_var("PALM_TEST_SECRET") };
    let rendered = res.captured.render();
    assert!(
        !rendered.contains("sk-do-not-leak"),
        "secret leaked: {rendered}"
    );
    assert!(!rendered.contains("PALM_TEST_SECRET"));
}

#[test]
#[cfg(any(target_os = "linux", target_os = "android"))]
fn stale_pidfile_sweep_reaps_the_group() {
    let dir = tempfile::tempdir().unwrap();
    let pidfile = dir.path().join("run.pid");
    let mut spec = exec().spec("sleep 30", dir.path(), env(dir.path()));
    spec.timeout = Duration::from_secs(60);
    spec.pidfile = Some(pidfile.clone());
    let ex = exec();
    let handle = std::thread::spawn(move || ex.run(&spec, &mut |_b: &[u8]| {}));
    // Wait for the pidfile, then sweep as the resume path would.
    let mut pid = None;
    for _ in 0..100 {
        if let Ok(s) = std::fs::read_to_string(&pidfile) {
            pid = s
                .split_whitespace()
                .next()
                .and_then(|s| s.parse::<u32>().ok());
            if pid.is_some() {
                break;
            }
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(pid.is_some(), "pidfile never appeared");
    assert_eq!(sweep_orphans(std::slice::from_ref(&pidfile)), 1);
    assert!(!pidfile.exists(), "sweep must consume the pidfile");
    let started = Instant::now();
    let res = handle.join().unwrap().unwrap();
    assert!(
        res.cancelled
            || res.timed_out
            || res.signal.is_some()
            || started.elapsed() < Duration::from_secs(5)
    );
    // The sleep must be gone: the tree kill hit the recorded pid. Only LIVE
    // members count - a SIGKILLed grandchild briefly stays zombie until init
    // reaps it, and kill(-pgid, 0) sees zombies as existing. Survivors are
    // printed with state so CI failure output names the leak.
    #[cfg(unix)]
    {
        let pgid = pid.unwrap() as i32;
        let mut alive = Vec::new();
        for e in std::fs::read_dir("/proc").unwrap() {
            let e = e.unwrap();
            let name = e.file_name();
            let name = name.to_string_lossy();
            if name.is_empty() || !name.chars().all(|c| c.is_ascii_digit()) {
                continue;
            }
            let Ok(stat) = std::fs::read_to_string(e.path().join("stat")) else {
                continue;
            };
            // "pid (comm) state ppid pgrp ...": comm may contain spaces and
            // parens, so split from the last ") ".
            let Some(rest) = stat.rsplit_once(") ") else {
                continue;
            };
            let mut f = rest.1.split_whitespace();
            let state = f.next().unwrap_or("?");
            let _ppid = f.next();
            let pgrp: i32 = f.next().and_then(|v| v.parse().ok()).unwrap_or(0);
            if pgrp == pgid && !matches!(state, "Z" | "X" | "x") {
                alive.push(format!("{name} state={state} stat={}", stat.trim()));
            }
        }
        assert!(alive.is_empty(), "group still has live members: {alive:?}");
    }
}

#[cfg(unix)]
#[test]
fn signal_death_reports_signal() {
    let dir = tempfile::tempdir().unwrap();
    let res = exec()
        .run(
            &exec().spec("kill -TERM $$", dir.path(), env(dir.path())),
            &mut |_b: &[u8]| {},
        )
        .unwrap();
    // sh reports 143 as its own exit code; the wrapper may see the signal.
    assert!(res.exit == Some(143) || res.signal == Some(15), "{res:?}");
}

// Keep the type honest on Windows hosts where sh lives in the pinned MSYS2.
#[cfg(windows)]
#[test]
fn sh_resolves_somewhere_sensible() {
    let sh = HostExecutor::new();
    let p = sh.sh().to_path_buf();
    assert!(
        p.ends_with("sh.exe") || p == std::path::Path::new("sh"),
        "{p:?}"
    );
}
