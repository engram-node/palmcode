//! edit-bench (deliverable K): drive the real agent loop (Engine + tools +
//! permissions) through the 20 edit tasks against a REAL provider and grade
//! the resulting workspace. Approvals are auto-answered "allow once"; the
//! measured metrics are pass rate + turns + tool-call mix + latency + tokens.
//!
//!   GRID_BASE_URL (default: http://127.0.0.1:8787/v1 for a local mock run)
//!   GRID_API_KEY  (never logged, never persisted)
//!   GRID_MODEL    (default: glm-latest)
//!
//! Run:  cargo run -p palmcode-bench --bin edit-bench --release [--task <id>] [--list]

include!("../edit_bench_tasks.rs");

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use palmcode_agent::{ApprovalScope, Engine, Event, EventSink, ProviderCfg};

const TIMEOUT: Duration = Duration::from_secs(180);

#[derive(Default)]
struct TaskSink {
    inner: Mutex<SinkState>,
}

#[derive(Default)]
struct SinkState {
    pending: Vec<u64>,        // approval request ids awaiting an answer
    finished: Option<String>, // finish_reason / "interrupted" / "error"
    tools: Vec<bool>,         // ok per executed tool call
    prompt_tokens: u64,
    completion_tokens: u64,
    error: Option<String>,
}

impl EventSink for TaskSink {
    fn on_batch(&self, events: Vec<Event>) {
        let mut st = self.inner.lock().unwrap();
        for e in events {
            match e {
                Event::ApprovalRequested { request_id, .. } => st.pending.push(request_id),
                Event::ToolFinished { ok, .. } => st.tools.push(ok),
                Event::MessageFinished {
                    finish_reason,
                    prompt_tokens,
                    completion_tokens,
                    ..
                } => {
                    st.finished = Some(finish_reason);
                    st.prompt_tokens += prompt_tokens.unwrap_or(0) as u64;
                    st.completion_tokens += completion_tokens.unwrap_or(0) as u64;
                }
                Event::Interrupted { .. } => st.finished = Some("interrupted".into()),
                Event::TurnError { code, detail, .. } => {
                    st.finished = Some("error".into());
                    st.error = Some(format!("{code}: {detail}"));
                }
                _ => {}
            }
        }
    }
}

fn verify(ws: &std::path::Path, expect: &[(&str, &str)]) -> Vec<String> {
    let mut fails = Vec::new();
    for (path, needle) in expect {
        let (neg, p) = match path.strip_prefix('!') {
            Some(r) => (true, r),
            None => (false, *path),
        };
        let content = std::fs::read_to_string(ws.join(p)).unwrap_or_default();
        let has = content.contains(needle);
        if (!neg && !has) || (neg && has) {
            fails.push(format!(
                "{path}: {} {:?}",
                if neg { "must not contain" } else { "missing" },
                needle
            ));
        }
    }
    fails
}

// sync main: Engine owns its runtime; nesting it inside a tokio runtime
// panics ("Cannot start a runtime from within a runtime").
fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--list") {
        for t in TASKS {
            println!("{}", t.id);
        }
        return;
    }
    let only: Option<String> = args
        .windows(2)
        .find(|w| w[0] == "--task")
        .map(|w| w[1].clone());

    let base = std::env::var("GRID_BASE_URL").unwrap_or_else(|_| "http://127.0.0.1:8787/v1".into());
    let key = std::env::var("GRID_API_KEY").expect("GRID_API_KEY (BYOK; env-only)");
    let model = std::env::var("GRID_MODEL").unwrap_or_else(|_| "glm-latest".into());

    let mut pass = 0usize;
    let mut run = 0usize;
    let mut total_time = Duration::ZERO;
    let mut total_completion = 0u64;
    let mut total_prompt = 0u64;
    let mut total_tools = 0usize;
    let mut total_tool_err = 0usize;

    for task in TASKS {
        if let Some(only) = &only
            && task.id != only
        {
            continue;
        }
        run += 1;
        let t0 = Instant::now();
        let dir = tempfile::tempdir().expect("tmp");
        let ws = dir.path().join("ws");
        for (p, content) in task.files {
            let full = ws.join(p);
            if let Some(par) = full.parent() {
                std::fs::create_dir_all(par).unwrap();
            }
            std::fs::write(full, content).unwrap();
        }

        let sink = Arc::new(TaskSink::default());
        let engine = Engine::start(&dir.path().join("b.db"), sink.clone() as Arc<dyn EventSink>)
            .expect("engine");
        engine
            .configure(ProviderCfg {
                base_url: base.clone(),
                api_key: key.clone(),
                main_model: model.clone(),
                fast_model: model.clone(),
                context_window: 202_752,
                max_output_tokens: 4096,
                sampling_json: None,
                extra_body_json: Some(
                    "{\"chat_template_kwargs\": {\"enable_thinking\": true}}".into(),
                ),
                continuous_usage: true,
                first_token_secs: 60,
                idle_secs: 120,
            })
            .expect("configure");
        engine
            .set_workspace(ws.to_str().expect("utf8 path"))
            .expect("workspace");

        let session = engine.create_session(task.id).expect("session");
        let mid = engine.send(&session.id, task.prompt).expect("send");

        // pump: answer approvals instantly, bail on timeout
        let deadline = Instant::now() + TIMEOUT;
        let mut fin: Option<String> = None;
        while fin.is_none() && Instant::now() < deadline {
            let (pend, done) = {
                let mut st = sink.inner.lock().unwrap();
                let done = st.finished.take();
                (std::mem::take(&mut st.pending), done)
            };
            for rid in pend {
                engine.resolve_approval(rid, true, ApprovalScope::Once);
            }
            fin = done;
            if fin.is_none() {
                std::thread::sleep(Duration::from_millis(100));
            }
        }
        let st = sink.inner.lock().unwrap();
        let elapsed = t0.elapsed();
        let fails = verify(&ws, task.expect);
        let ok_run = fin.as_deref() == Some("stop") && fails.is_empty();
        let tools = st.tools.len();
        let tool_err = st.tools.iter().filter(|ok| !**ok).count();
        total_time += elapsed;
        total_completion += st.completion_tokens;
        total_prompt += st.prompt_tokens;
        total_tools += tools;
        total_tool_err += tool_err;
        if ok_run {
            pass += 1;
            println!(
                "[PASS] {:<16} {:.1}s · tools {} ({} err) · out {} tok",
                task.id,
                elapsed.as_secs_f64(),
                tools,
                tool_err,
                st.completion_tokens
            );
        } else {
            let reason = match (&fin, &fails, &st.error) {
                (None, _, _) => "TIMEOUT".to_string(),
                (_, e, _) if !e.is_empty() => format!("verify: {e:?}"),
                (_, _, Some(err)) => format!("engine: {err}"),
                (f, _, _) => format!("finish={f:?}"),
            };
            println!(
                "[FAIL] {:<16} {:.1}s · {} · mid={mid}",
                task.id,
                elapsed.as_secs_f64(),
                reason
            );
        }
        drop(st);
        drop(engine);
    }

    println!(
        "\n=== edit-bench: {pass}/{run} passed ({:.0}%)",
        100.0 * pass as f64 / run.max(1) as f64
    );
    println!(
        "mean {:.1}s/task · {} tool calls ({} errors) · tokens in {} / out {}",
        total_time.as_secs_f64() / run.max(1) as f64,
        total_tools,
        total_tool_err,
        total_prompt,
        total_completion,
    );
    std::process::exit(if pass == run && run > 0 { 0 } else { 1 });
}
