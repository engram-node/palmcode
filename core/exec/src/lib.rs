//! palmcode-exec -- Executor trait + implementations (NativeProc, Host).
//!
//! Tools never call `std::process` directly (repo convention): every child
//! goes through [`Executor`], which owns the M3-D process contract --
//! fresh `sh -c` per call, new session/process group (`setsid`, unix), stdin
//! closed, allowlisted env (nothing from the parent process leaks), timeout
//! kill-tree (SIGTERM -> 2 s -> SIGKILL), PDEATHSIG (unix), merged output
//! stream with head/tail capture + full spill file, and a pidfile so a
//! kill-mid-command can be swept without orphans (ADR 0014).

mod host;
mod native;

pub mod applets;

pub use applets::BUSYBOX_APPLETS;
pub use host::HostExecutor;
pub use native::NativeProcExecutor;

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("spawn failed: {0}")]
    Spawn(String),
    #[error("bad spec: {0}")]
    Spec(String),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

/// SIGTERM grace before SIGKILL (deliverable D).
pub const KILL_GRACE: Duration = Duration::from_secs(2);
/// Capture window kept inline: head + tail of the merged stream.
pub const CAPTURE_HEAD: usize = 32 * 1024;
pub const CAPTURE_TAIL: usize = 32 * 1024;

/// Merged stdout+stderr chunks as they arrive (live stream for tool cards).
pub trait OutputSink {
    fn on_chunk(&mut self, bytes: &[u8]);
}

impl<F: FnMut(&[u8])> OutputSink for F {
    fn on_chunk(&mut self, bytes: &[u8]) {
        self(bytes)
    }
}

/// Cancellation flag shared with the UI kill button.
#[derive(Clone, Default)]
pub struct Cancel(std::sync::Arc<AtomicBool>);

impl Cancel {
    pub fn cancel(&self) {
        self.0.store(true, Ordering::SeqCst);
    }
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }
}

/// Allowlisted child environment. The parent process env is never inherited:
/// key material cannot reach a child env (repo rule).
#[derive(Clone, Debug)]
pub struct EnvSpec {
    pub path: String,
    pub home: PathBuf,
    pub tmpdir: PathBuf,
    pub extra: BTreeMap<String, String>,
}

impl EnvSpec {
    /// Deterministic (sorted) variable list: PATH/HOME/TMPDIR/LANG/TERM plus
    /// explicit `extra`. `extra` may override the fixed keys except PATH.
    pub fn vars(&self) -> Vec<(String, String)> {
        let mut m: BTreeMap<String, String> = BTreeMap::new();
        m.insert("HOME".into(), self.home.to_string_lossy().into_owned());
        m.insert("LANG".into(), "C.UTF-8".into());
        m.insert("TERM".into(), "dumb".into());
        m.insert("TMPDIR".into(), self.tmpdir.to_string_lossy().into_owned());
        for (k, v) in &self.extra {
            if !k.starts_with("PALMCODE_LLM_") && k != "GIT_EXTERNAL_DIFF" {
                m.insert(k.clone(), v.clone());
            }
        }
        m.insert("PATH".into(), self.path.clone());
        m.into_iter().collect()
    }
}

/// Git command-scope configuration outranks repository and global config.
/// Paths must refer to an app-controlled empty file and empty directory.
pub fn git_environment(
    env: &EnvSpec,
    empty_config: &Path,
    empty_hooks: &Path,
) -> Vec<(String, String)> {
    let mut vars: BTreeMap<_, _> = env.vars().into_iter().collect();
    vars.retain(|k, _| !k.starts_with("GIT_CONFIG_") && k != "GIT_EXTERNAL_DIFF");
    vars.insert("GIT_CONFIG_NOSYSTEM".into(), "1".into());
    vars.insert(
        "GIT_CONFIG_GLOBAL".into(),
        empty_config.to_string_lossy().into_owned(),
    );
    let settings = [
        ("core.fsmonitor", "false".to_owned()),
        ("core.hooksPath", empty_hooks.to_string_lossy().into_owned()),
        ("core.pager", "cat".to_owned()),
        ("diff.external", String::new()),
        ("core.sshCommand", String::new()),
        ("credential.helper", String::new()),
        ("safe.directory", env.home.to_string_lossy().into_owned()),
    ];
    vars.insert("GIT_CONFIG_COUNT".into(), settings.len().to_string());
    for (n, (key, value)) in settings.into_iter().enumerate() {
        vars.insert(format!("GIT_CONFIG_KEY_{n}"), key.into());
        vars.insert(format!("GIT_CONFIG_VALUE_{n}"), value);
    }
    vars.into_iter().collect()
}

#[derive(Clone, Copy)]
pub struct CaptureLimits {
    pub head: usize,
    pub tail: usize,
}

impl Default for CaptureLimits {
    fn default() -> Self {
        Self {
            head: CAPTURE_HEAD,
            tail: CAPTURE_TAIL,
        }
    }
}

/// Head/tail capture of the merged stream; the full stream goes to the spill
/// file when one is configured, so nothing large is held in memory.
#[derive(Debug)]
pub struct Captured {
    pub head: Vec<u8>,
    pub tail: Vec<u8>,
    pub total_bytes: u64,
    pub total_lines: u64,
}

impl Captured {
    /// Model-facing rendering: full text when small, head + elision + tail
    /// when large.
    pub fn render(&self) -> String {
        let total = self.total_bytes as usize;
        if total <= self.head.len() + self.tail.len() {
            let mut out = self.head.clone();
            if total > self.head.len() {
                out.extend_from_slice(&self.tail[..total - self.head.len()]);
            }
            return String::from_utf8_lossy(&out).into_owned();
        }
        let elided = total - self.head.len() - self.tail.len();
        format!(
            "{}\n...[{elided} bytes elided; full output in spill file]...\n{}",
            String::from_utf8_lossy(&self.head),
            String::from_utf8_lossy(&self.tail)
        )
    }
}

struct Accumulator {
    limits: CaptureLimits,
    head: Vec<u8>,
    tail: Vec<u8>,
    total_bytes: u64,
    total_lines: u64,
}

impl Accumulator {
    fn new(limits: CaptureLimits) -> Self {
        Self {
            limits,
            head: Vec::new(),
            tail: Vec::new(),
            total_bytes: 0,
            total_lines: 0,
        }
    }

    fn push(&mut self, chunk: &[u8]) {
        self.total_bytes += chunk.len() as u64;
        self.total_lines += chunk.iter().filter(|b| **b == b'\n').count() as u64;
        let head_room = self.limits.head.saturating_sub(self.head.len());
        self.head
            .extend_from_slice(&chunk[..chunk.len().min(head_room)]);
        self.tail.extend_from_slice(chunk);
        if self.tail.len() > self.limits.tail {
            let excess = self.tail.len() - self.limits.tail;
            self.tail.drain(..excess);
        }
    }

    fn finish(self) -> Captured {
        Captured {
            head: self.head,
            tail: self.tail,
            total_bytes: self.total_bytes,
            total_lines: self.total_lines,
        }
    }
}

#[derive(Debug)]
pub struct ExecResult {
    /// Exit code when the child exited normally.
    pub exit: Option<i32>,
    /// Unix signal number when the child was signaled (else None).
    pub signal: Option<i32>,
    pub timed_out: bool,
    pub cancelled: bool,
    pub captured: Captured,
    /// Full merged stream spill file, when the spec asked for one.
    pub spill: Option<PathBuf>,
    pub duration: Duration,
}

/// One child process request. `argv[0]` is resolved via `env.path`.
pub struct ExecSpec {
    pub argv: Vec<String>,
    pub cwd: PathBuf,
    pub env: EnvSpec,
    pub timeout: Duration,
    pub capture: CaptureLimits,
    pub spill_dir: Option<PathBuf>,
    /// Written as `<pid>\n` while running and removed on completion; a stale
    /// file means the app died mid-run and its group must be swept.
    pub pidfile: Option<PathBuf>,
    pub cancel: Cancel,
}

impl ExecSpec {
    /// The bash tool contract: fresh `sh -c`, one call per command.
    pub fn shell(sh: &Path, script: &str, cwd: &Path, env: EnvSpec) -> Self {
        Self {
            argv: vec![
                sh.to_string_lossy().into_owned(),
                "-c".into(),
                script.to_string(),
            ],
            cwd: cwd.to_path_buf(),
            env,
            timeout: Duration::from_secs(120),
            capture: CaptureLimits::default(),
            spill_dir: None,
            pidfile: None,
            cancel: Cancel::default(),
        }
    }
}

pub trait Executor: Send + Sync {
    /// The shell the bash tool wraps commands in.
    fn sh(&self) -> &Path;
    /// Run to completion, streaming merged output to `sink`. Blocking.
    fn run(&self, spec: &ExecSpec, sink: &mut dyn OutputSink) -> Result<ExecResult>;
}

/// Force-kill any process groups recorded in leftover pidfiles (resume path
/// after kill-mid-command). Returns how many groups were signalled.
/// shortcut: a stale pidfile can hit a reused pid, record process start-time
/// in the pidfile if pidfiles ever live longer than one session.
pub fn sweep_orphans(pidfiles: &[PathBuf]) -> usize {
    let mut swept = 0;
    for pf in pidfiles {
        if let Some(pid) = std::fs::read_to_string(pf)
            .ok()
            .and_then(|s| s.trim().parse::<u32>().ok())
            && kill_group_force(pid)
        {
            swept += 1;
        }
        let _ = std::fs::remove_file(pf);
    }
    swept
}

// ---------------------------------------------------------------------------
// Shared spawn/collect/kill machinery
// ---------------------------------------------------------------------------

static SPILL_SEQ: AtomicU64 = AtomicU64::new(1);

struct GitConfigDir(PathBuf);
impl Drop for GitConfigDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

struct Spill {
    file: std::fs::File,
    path: PathBuf,
}

impl Spill {
    fn create(dir: &Path) -> Result<Self> {
        std::fs::create_dir_all(dir)?;
        let path = dir.join(format!(
            "execspill-{}-{}.log",
            std::process::id(),
            SPILL_SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        let file = std::fs::File::create(&path)?;
        Ok(Self { file, path })
    }

    fn write(&mut self, chunk: &[u8]) -> Result<()> {
        self.file.write_all(chunk)?;
        Ok(())
    }
}

pub(crate) fn run(spec: &ExecSpec, sink: &mut dyn OutputSink) -> Result<ExecResult> {
    let argv0 = spec
        .argv
        .first()
        .ok_or_else(|| Error::Spec("empty argv".into()))?;
    let started = Instant::now();
    // A private per-call directory prevents workspace config from supplying
    // hooks or global configuration. Remove it when the call finishes.
    let git_dir = spec.env.tmpdir.join(format!(
        "git-config-{}-{}",
        std::process::id(),
        SPILL_SEQ.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&git_dir)?;
    let git_guard = GitConfigDir(git_dir);
    let empty_config = git_guard.0.join("config");
    let empty_hooks = git_guard.0.join("hooks");
    std::fs::write(&empty_config, [])?;
    std::fs::create_dir(&empty_hooks)?;
    let mut cmd = Command::new(argv0);
    cmd.args(&spec.argv[1..])
        .current_dir(&spec.cwd)
        .env_clear()
        .envs(git_environment(&spec.env, &empty_config, &empty_hooks))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    configure(&mut cmd);
    let mut child = cmd
        .spawn()
        .map_err(|e| Error::Spawn(format!("{argv0}: {e}")))?;
    let pid = child.id();
    if let Some(pf) = &spec.pidfile {
        let _ = std::fs::write(pf, format!("{pid}\n"));
    }

    // Two reader threads merge into one channel; arrival order is the merge.
    let (tx, rx) = mpsc::channel::<Vec<u8>>();
    let mut pipes: Vec<Box<dyn Read + Send>> = Vec::new();
    if let Some(p) = child.stdout.take() {
        pipes.push(Box::new(p));
    }
    if let Some(p) = child.stderr.take() {
        pipes.push(Box::new(p));
    }
    for mut pipe in pipes {
        let tx = tx.clone();
        std::thread::spawn(move || {
            let mut buf = [0u8; 8192];
            loop {
                match pipe.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        if tx.send(buf[..n].to_vec()).is_err() {
                            break;
                        }
                    }
                }
            }
        });
    }
    drop(tx);

    let deadline = started + spec.timeout;
    let mut acc = Accumulator::new(spec.capture);
    let mut spill = match &spec.spill_dir {
        Some(d) => Some(Spill::create(d)?),
        None => None,
    };
    let mut timed_out = false;
    let mut cancelled = false;
    let mut status: Option<ExitStatus> = child.try_wait()?;
    let mut last_data = Instant::now();

    loop {
        if status.is_none() {
            if !timed_out && Instant::now() >= deadline {
                terminate_tree(pid);
                timed_out = true;
            }
            if !cancelled && spec.cancel.is_cancelled() {
                terminate_tree(pid);
                cancelled = true;
            }
        }
        match rx.recv_timeout(Duration::from_millis(50)) {
            Ok(chunk) => {
                sink.on_chunk(&chunk);
                acc.push(&chunk);
                if let Some(s) = &mut spill {
                    s.write(&chunk)?;
                }
                last_data = Instant::now();
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if status.is_none() {
                    status = child.try_wait()?;
                }
                // Child exited but a grandchild still holds the pipes open:
                // the bash contract says command done = tree done (no
                // leftovers), so reap stragglers and stop waiting.
                if status.is_some() && last_data.elapsed() > Duration::from_millis(200) {
                    terminate_tree(pid);
                    break;
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }

    // Drain whatever arrived while terminating (best effort).
    while let Ok(chunk) = rx.try_recv() {
        sink.on_chunk(&chunk);
        acc.push(&chunk);
        if let Some(s) = &mut spill {
            s.write(&chunk)?;
        }
    }
    if status.is_none() {
        status = Some(child.wait()?);
    }
    if let Some(pf) = &spec.pidfile {
        let _ = std::fs::remove_file(pf);
    }

    Ok(ExecResult {
        exit: status.and_then(|s| s.code()),
        signal: signaled(status),
        timed_out,
        cancelled,
        captured: acc.finish(),
        spill: spill.map(|s| s.path),
        duration: started.elapsed(),
    })
}

#[cfg(unix)]
fn signaled(status: Option<ExitStatus>) -> Option<i32> {
    use std::os::unix::process::ExitStatusExt;
    status.and_then(|s| s.signal())
}

#[cfg(not(unix))]
fn signaled(_status: Option<ExitStatus>) -> Option<i32> {
    None
}

/// Spawn configuration: new session/process group (unix: one kill(-pid)
/// reaps the whole tree per ADR 0014) + PDEATHSIG so the tree dies with the
/// app even if the sweep never runs.
#[cfg(unix)]
fn configure(cmd: &mut Command) {
    use std::os::unix::process::CommandExt;
    let parent = std::process::id();
    unsafe {
        cmd.pre_exec(move || {
            if libc::setsid() == -1 {
                return Err(std::io::Error::last_os_error());
            }
            libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL);
            // Guard the fork->prctl race: if our parent died in between,
            // PDEATHSIG was never armed.
            if libc::getppid() as u32 != parent {
                libc::raise(libc::SIGKILL);
            }
            Ok(())
        });
    }
}

#[cfg(windows)]
fn configure(cmd: &mut Command) {
    use std::os::windows::process::CommandExt;
    // CREATE_NEW_PROCESS_GROUP: taskkill /T walks the tree from the child.
    cmd.creation_flags(0x00000200);
}

/// TERM the tree, 2 s grace, KILL (deliverable D). Windows has no tree TERM;
/// taskkill /T /F is the tree kill there (host-only path).
fn terminate_tree(pid: u32) {
    #[cfg(unix)]
    {
        if !tree_signal(pid, libc::SIGTERM) {
            return;
        }
        let give_up = Instant::now() + KILL_GRACE;
        while Instant::now() < give_up {
            if !tree_alive(pid) {
                return;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        tree_signal(pid, libc::SIGKILL);
    }
    #[cfg(windows)]
    {
        let _ = Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
}

/// Force-kill a recorded group. Returns true when a signal was delivered.
fn kill_group_force(pid: u32) -> bool {
    #[cfg(unix)]
    {
        // Direct negative-pgid form: busybox kill has no `--` (ADR 0014).
        let hit = unsafe { libc::kill(-(pid as i32), libc::SIGKILL) == 0 };
        let hit_self = unsafe { libc::kill(pid as i32, libc::SIGKILL) == 0 };
        hit || hit_self
    }
    #[cfg(windows)]
    {
        Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }
}

#[cfg(unix)]
fn tree_signal(pid: u32, sig: i32) -> bool {
    unsafe { libc::kill(-(pid as i32), sig) == 0 }
}

#[cfg(unix)]
fn tree_alive(pid: u32) -> bool {
    let alive = unsafe { libc::kill(-(pid as i32), 0) };
    alive == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn git_overrides_and_credentials_cannot_be_injected() {
        let env = EnvSpec {
            path: "bin".into(),
            home: "workspace".into(),
            tmpdir: "tmp".into(),
            extra: [
                "PALMCODE_LLM_API_KEY",
                "PALMCODE_LLM_BASE_URL",
                "PALMCODE_LLM_MODEL",
                "GIT_EXTERNAL_DIFF",
                "GIT_CONFIG_KEY_99",
                "GIT_CONFIG_COUNT",
            ]
            .into_iter()
            .map(|k| (k.into(), "untrusted".into()))
            .collect(),
        };
        let vars: BTreeMap<_, _> = git_environment(&env, Path::new("empty"), Path::new("hooks"))
            .into_iter()
            .collect();
        assert!(!vars.keys().any(|k| k.starts_with("PALMCODE_LLM_")
            || k == "GIT_EXTERNAL_DIFF"
            || k == "GIT_CONFIG_KEY_99"));
        assert_eq!(vars["GIT_CONFIG_NOSYSTEM"], "1");
        assert_eq!(vars["GIT_CONFIG_GLOBAL"], "empty");
        assert_eq!(vars["GIT_CONFIG_COUNT"], "7");
        for (n, (key, value)) in [
            ("core.fsmonitor", "false"),
            ("core.hooksPath", "hooks"),
            ("core.pager", "cat"),
            ("diff.external", ""),
            ("core.sshCommand", ""),
            ("credential.helper", ""),
            ("safe.directory", "workspace"),
        ]
        .into_iter()
        .enumerate()
        {
            assert_eq!(vars[&format!("GIT_CONFIG_KEY_{n}")], key);
            assert_eq!(vars[&format!("GIT_CONFIG_VALUE_{n}")], value);
        }
    }

    #[test]
    fn env_is_an_allowlist_sorted_and_deterministic() {
        let env = EnvSpec {
            path: "/farm/bin".into(),
            home: "/home/u".into(),
            tmpdir: "/tmp".into(),
            extra: BTreeMap::from([("FOO".to_string(), "1".to_string())]),
        };
        let vars = env.vars();
        let keys: Vec<&str> = vars.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(keys, vec!["FOO", "HOME", "LANG", "PATH", "TERM", "TMPDIR"]);
        // PATH cannot be overridden through extra.
        let env2 = EnvSpec {
            extra: BTreeMap::from([("PATH".to_string(), "evil".to_string())]),
            ..env.clone()
        };
        assert!(
            env2.vars()
                .iter()
                .any(|(k, v)| k == "PATH" && v == "/farm/bin")
        );
    }

    #[test]
    fn accumulator_head_tail_and_render() {
        let mut acc = Accumulator::new(CaptureLimits { head: 8, tail: 8 });
        acc.push(b"abc");
        acc.push(b"defghijklmnopqrstuvwxyz"); // 23 -> total 26
        let c = acc.finish();
        assert_eq!(c.head, b"abcdefgh");
        assert_eq!(c.tail, b"stuvwxyz");
        assert_eq!(c.total_bytes, 26);
        let r = c.render();
        assert!(r.starts_with("abcdefgh"));
        assert!(r.ends_with("stuvwxyz"));
        assert!(r.contains("10 bytes elided"));

        let mut small = Accumulator::new(CaptureLimits { head: 8, tail: 8 });
        small.push(b"hello\n");
        let c = small.finish();
        assert_eq!(c.render(), "hello\n");
        assert_eq!(c.total_lines, 1);
    }
}
