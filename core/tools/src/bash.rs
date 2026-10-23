//! Bash tool (deliverable D): one fresh `sh -c` per call through the
//! `palmcode_exec::Executor` contract -- new session/process group (one
//! kill(-pgid) reaps the tree), stdin closed, allowlisted env (HOME =
//! workspace root), timeout kill-tree (SIGTERM -> 2 s -> SIGKILL), merged
//! stdout+stderr streamed live to the sink, head/tail capture + full spill
//! file, pidfile so kill-mid-command leaves no orphans (ADR 0014/0015).
//!
//! Scripts with `#!/usr/bin/env X` shebangs do not exec on Android (no
//! /usr/bin/env): invoke scripts as `sh script`, never `./script` (ADR 0014).

use std::path::PathBuf;
use std::time::Duration;

use serde::Deserialize;

use crate::Spiller;
use crate::caps::Capped;
use crate::error::{Result, ToolError};

/// Per-session bash runtime: the executor, cwd and sanitized env are fixed
/// by the host layer (agent wires them; tests use HostExecutor).
pub struct Session {
    pub exec: std::sync::Arc<dyn palmcode_exec::Executor>,
    pub cwd: PathBuf,
    pub env: palmcode_exec::EnvSpec,
}

/// Live merged-output sink for one bash call (batched by the caller).
pub type Sink<'a> = std::cell::RefCell<Option<&'a mut dyn FnMut(&[u8])>>;

/// One call's runtime context: cancellation (UI kill button) and the live
/// output sink (interior mutability: `run_tool` takes `&Ctx`).
pub struct BashCtx<'a> {
    pub session: &'a Session,
    pub cancel: &'a palmcode_exec::Cancel,
    pub sink: Sink<'a>,
}

#[derive(Deserialize)]
struct Args {
    command: String,
    #[serde(default)]
    timeout_ms: Option<u64>,
    #[serde(default)]
    #[allow(dead_code)] // prompt-quality hint; not execution-relevant
    description: Option<String>,
}

/// Default / max wall-clock timeout (deliverable D: 120 s / 600 s).
pub const TIMEOUT_DEFAULT_MS: u64 = 120_000;
pub const TIMEOUT_MAX_MS: u64 = 600_000;

pub fn schema() -> serde_json::Value {
    serde_json::json!({
        "name": "bash",
        "description": "Run one shell command in the workspace (fresh sh -c per call; \
    merged stdout+stderr streams live; the tree is killed at timeout). Environment is \
    allowlisted: HOME=workspace, no parent env. Invoke scripts as `sh script` -- \
    `#!/usr/bin/env` shebangs do not exec on this platform. Kill-mid-command leaves no \
    orphans: the whole process group dies and nothing is re-run on resume.",
        "parameters": {
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "description": "The shell command to run (sh -c semantics)."
                },
                "timeout_ms": {
                    "type": "integer",
                    "description": "Wall-clock timeout in ms (default 120000, max 600000)."
                },
                "description": {
                    "type": "string",
                    "description": "5-10 word note on why this command runs."
                }
            },
            "required": ["command"]
        }
    })
}

pub fn run(args_json: &str, b: &BashCtx, spill: &Spiller) -> Result<Capped> {
    let args: Args = serde_json::from_str(args_json)
        .map_err(|e| ToolError::Guard(format!("invalid bash args: {e}")))?;
    if args.command.trim().is_empty() {
        return Err(ToolError::Guard("bash: empty command".into()));
    }
    if args.command.len() > 100_000 {
        return Err(ToolError::Guard("bash: command too long".into()));
    }
    let timeout_ms = args
        .timeout_ms
        .unwrap_or(TIMEOUT_DEFAULT_MS)
        .clamp(1_000, TIMEOUT_MAX_MS);

    let mut spec = palmcode_exec::ExecSpec::shell(
        b.session.exec.sh(),
        &args.command,
        &b.session.cwd,
        b.session.env.clone(),
    );
    spec.timeout = Duration::from_millis(timeout_ms);
    spec.spill_dir = Some(spill.dir().to_path_buf());
    spec.pidfile = Some(spill.dir().join(format!(
        "bash-{}-{}.pid",
        std::process::id(),
        PID_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    )));
    spec.cancel = b.cancel.clone();

    let res = {
        let mut sink = |chunk: &[u8]| {
            if let Some(s) = b.sink.borrow_mut().as_mut() {
                s(chunk);
            }
        };
        b.session.exec.run(&spec, &mut sink)
    }
    .map_err(|e| ToolError::Guard(format!("bash: {e}")))?;

    let status = status_line(&res, timeout_ms);
    let mut text = res.captured.render();
    if !text.ends_with('\n') && !text.is_empty() {
        text.push('\n');
    }
    text.push_str(&status);

    Ok(Capped {
        text,
        total_lines: res.captured.total_lines as usize,
        total_bytes: res.captured.total_bytes as usize,
        spilled_to: res.spill,
    })
}

static PID_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

fn status_line(res: &palmcode_exec::ExecResult, timeout_ms: u64) -> String {
    let secs = res.duration.as_millis();
    if res.cancelled {
        format!("[cancelled after {secs} ms · process tree killed]")
    } else if res.timed_out {
        format!("[timed out after {timeout_ms} ms · process tree killed]")
    } else if let Some(code) = res.exit {
        format!("[exit {code} · {secs} ms]")
    } else if let Some(sig) = res.signal {
        format!("[killed by signal {sig} · {secs} ms]")
    } else {
        format!("[done · {secs} ms]")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::caps::Spiller;
    use crate::{Ctx, ReadCache, RootSet};

    fn world() -> (
        tempfile::TempDir,
        RootSet,
        ReadCache,
        Spiller,
        Session,
        palmcode_exec::Cancel,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let roots = RootSet::new(vec![dir.path().to_path_buf()]).unwrap();
        let reads = ReadCache::new();
        let spill = Spiller::new(dir.path().join("spill")).unwrap();
        let env = palmcode_exec::EnvSpec {
            path: std::env::var("PATH").unwrap_or_default(),
            home: dir.path().to_path_buf(),
            tmpdir: dir.path().to_path_buf(),
            extra: Default::default(),
        };
        let session = Session {
            exec: std::sync::Arc::new(palmcode_exec::HostExecutor::new()),
            cwd: dir.path().to_path_buf(),
            env,
        };
        (
            dir,
            roots,
            reads,
            spill,
            session,
            palmcode_exec::Cancel::default(),
        )
    }

    #[test]
    fn echo_runs_and_reports_exit() {
        let (_d, roots, reads, spill, session, cancel) = world();
        let ctx = BashCtx {
            session: &session,
            cancel: &cancel,
            sink: std::cell::RefCell::new(None),
        };
        let tctx = Ctx {
            roots: &roots,
            reads: &reads,
            spill: &spill,
            bash: Some(&ctx),
        };
        let out = crate::run_tool("bash", r#"{"command":"echo hi"}"#, &tctx).unwrap();
        assert!(out.text.contains("hi"), "{:?}", out.text);
        assert!(out.text.contains("[exit 0"), "{:?}", out.text);
    }

    #[test]
    fn exit_code_and_spill() {
        let (_d, roots, reads, spill, session, cancel) = world();
        let ctx = BashCtx {
            session: &session,
            cancel: &cancel,
            sink: std::cell::RefCell::new(None),
        };
        let tctx = Ctx {
            roots: &roots,
            reads: &reads,
            spill: &spill,
            bash: Some(&ctx),
        };
        let out = crate::run_tool("bash", r#"{"command":"echo x; exit 7"}"#, &tctx).unwrap();
        assert!(out.text.contains("[exit 7"), "{:?}", out.text);
        let spill_path = out.spilled_to.expect("spill file");
        assert!(std::fs::read_to_string(spill_path).unwrap().contains("x"));
    }

    #[test]
    fn timeout_kills_the_tree_fast() {
        let (_d, roots, reads, spill, session, cancel) = world();
        let ctx = BashCtx {
            session: &session,
            cancel: &cancel,
            sink: std::cell::RefCell::new(None),
        };
        let tctx = Ctx {
            roots: &roots,
            reads: &reads,
            spill: &spill,
            bash: Some(&ctx),
        };
        let started = std::time::Instant::now();
        let out =
            crate::run_tool("bash", r#"{"command":"sleep 30","timeout_ms":1000}"#, &tctx).unwrap();
        assert!(out.text.contains("timed out"), "{:?}", out.text);
        assert!(started.elapsed() < Duration::from_secs(10));
    }

    #[test]
    fn sink_gets_the_live_stream() {
        let (_d, roots, reads, spill, session, cancel) = world();
        let mut chunks: Vec<u8> = Vec::new();
        let mut sink_fn = |b: &[u8]| chunks.extend_from_slice(b);
        let ctx = BashCtx {
            session: &session,
            cancel: &cancel,
            sink: std::cell::RefCell::new(Some(&mut sink_fn as &mut dyn FnMut(&[u8]))),
        };
        let tctx = Ctx {
            roots: &roots,
            reads: &reads,
            spill: &spill,
            bash: Some(&ctx),
        };
        crate::run_tool("bash", r#"{"command":"echo live"}"#, &tctx).unwrap();
        assert_eq!(String::from_utf8_lossy(&chunks), "live\n");
    }

    #[test]
    fn guard_errors_are_model_readable() {
        let (_d, roots, reads, spill, session, cancel) = world();
        let ctx = BashCtx {
            session: &session,
            cancel: &cancel,
            sink: std::cell::RefCell::new(None),
        };
        let tctx = Ctx {
            roots: &roots,
            reads: &reads,
            spill: &spill,
            bash: Some(&ctx),
        };
        assert!(crate::run_tool("bash", r#"{"command":"  "}"#, &tctx).is_err());
        assert!(crate::run_tool("bash", "not json", &tctx).is_err());
    }
}
