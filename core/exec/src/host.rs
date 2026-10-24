//! Host executor (dev machine, CI): the platform `sh` -- MSYS2 sh on the
//! Windows dev host, /bin/sh on Linux CI. Mirrors the on-device process
//! contract so bash-tool behavior is testable without a phone.

use std::path::{Path, PathBuf};

use crate::{EnvSpec, ExecResult, ExecSpec, Executor, OutputSink, Result, run};

pub struct HostExecutor {
    sh: PathBuf,
}

impl HostExecutor {
    /// Resolution: `PALMCODE_HOST_SH`, then `sh` on PATH, then the pinned
    /// MSYS2 install path (docs/decisions/0002 toolchain pins).
    pub fn new() -> Self {
        let sh = std::env::var_os("PALMCODE_HOST_SH")
            .map(PathBuf::from)
            .or_else(|| which("sh"))
            .unwrap_or_else(|| PathBuf::from("sh"));
        Self { sh }
    }

    pub fn with_sh(sh: impl Into<PathBuf>) -> Self {
        Self { sh: sh.into() }
    }

    /// Convenience spec: `sh -c <script>` with defaults.
    pub fn spec(&self, script: &str, cwd: &Path, env: EnvSpec) -> ExecSpec {
        ExecSpec::shell(&self.sh, script, cwd, env)
    }
}

impl Default for HostExecutor {
    fn default() -> Self {
        Self::new()
    }
}

impl Executor for HostExecutor {
    fn sh(&self) -> &Path {
        &self.sh
    }

    fn run(&self, spec: &ExecSpec, sink: &mut dyn OutputSink) -> Result<ExecResult> {
        run(spec, sink)
    }
}

#[cfg(windows)]
fn which(sh: &str) -> Option<PathBuf> {
    let p = PathBuf::from(format!("{sh}.exe"));
    if p.exists() {
        return Some(p);
    }
    let msys = PathBuf::from("C:/msys64/usr/bin/sh.exe");
    if msys.exists() { Some(msys) } else { None }
}

#[cfg(not(windows))]
fn which(_sh: &str) -> Option<PathBuf> {
    None
}
