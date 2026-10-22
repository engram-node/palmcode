//! On-device executor (Android): the packaged userland shell and applet-farm
//! PATH (deliverable B wires the paths). Same process contract as
//! [`crate::HostExecutor`] -- that is the point of the trait.

use std::path::{Path, PathBuf};

use crate::{EnvSpec, ExecResult, ExecSpec, Executor, OutputSink, Result, run};

pub struct NativeProcExecutor {
    sh: PathBuf,
    env: EnvSpec,
}

impl NativeProcExecutor {
    /// `sh` is the packaged busybox (or a symlink farm entry next to it);
    /// `env` carries the on-device PATH/HOME/TMPDIR allowlist.
    pub fn new(sh: impl Into<PathBuf>, env: EnvSpec) -> Self {
        Self { sh: sh.into(), env }
    }

    pub fn env(&self) -> &EnvSpec {
        &self.env
    }

    /// Convenience spec: `sh -c <script>` in the workspace with the device
    /// env. Timeout/cancel/pidfile are set by the bash tool layer.
    pub fn spec(&self, script: &str, cwd: &Path) -> ExecSpec {
        ExecSpec::shell(&self.sh, script, cwd, self.env.clone())
    }
}

impl Executor for NativeProcExecutor {
    fn sh(&self) -> &Path {
        &self.sh
    }

    fn run(&self, spec: &ExecSpec, sink: &mut dyn OutputSink) -> Result<ExecResult> {
        run(spec, sink)
    }
}
