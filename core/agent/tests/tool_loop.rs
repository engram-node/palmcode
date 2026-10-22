//! M2 tool loop against the mock vLLM server: fixtures replayed with the
//! legacy names rewritten to M2 tool names (read_file -> read, etc.), one
//! script per request, results fed back into the wire history.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use palmcode_agent::{ApprovalScope, Engine, Event, EventSink, ProviderCfg};
use palmcode_llm::mock::{ChatScript, MockConfig, serve};

struct Collector(Mutex<Vec<Event>>);
impl EventSink for Collector {
    fn on_batch(&self, events: Vec<Event>) {
        self.0.lock().unwrap().extend(events);
    }
}
impl Collector {
    fn has(&self, pred: impl Fn(&Event) -> bool) -> bool {
        self.0.lock().unwrap().iter().any(pred)
    }
    fn count(&self, pred: impl Fn(&Event) -> bool) -> usize {
        self.0.lock().unwrap().iter().filter(|e| pred(e)).count()
    }
}

fn wait_for(c: &Collector, pred: impl Fn(&Event) -> bool, what: &str) {
    for _ in 0..800 {
        if c.has(&pred) {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!(
        "timed out waiting for {what}; events: {:?}",
        c.0.lock().unwrap()
    );
}

fn cfg(base: String) -> ProviderCfg {
    ProviderCfg {
        base_url: base,
        api_key: "sk-mock".into(),
        main_model: "mock-glm".into(),
        fast_model: "mock-glm-flash".into(),
        context_window: 128_000,
        max_output_tokens: 512,
        sampling_json: None,
        extra_body_json: None,
        continuous_usage: false,
        first_token_secs: 5,
        idle_secs: 2,
    }
}

/// fixtue with legacy gateway tool names renamed to M2 names
fn fixture(name: &str, renames: &[(&str, &str)]) -> Vec<u8> {
    let mut text: String = std::fs::read_to_string(format!(
        "{}/../llm/fixtures/vllm/{name}",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap();
    for (from, to) in renames {
        text = text.replace(from, to);
    }
    text.into_bytes()
}

const FOLLOWUP: &[(&str, &str)] = &[];
const TOOL_CALL_READ: &[(&str, &str)] = &[("read_file", "read")];

const TOOL_CALL_WRITE: &[(&str, &str)] = &[("read_file", "write")];

const PARALLEL_READS: &[(&str, &str)] = &[("read_file", "read"), ("list_files", "ls")];

#[allow(dead_code)] // server/rt keep the listener alive for the test's scope
struct Rig {
    _dir: tempfile::TempDir,
    c: Arc<Collector>,
    engine: Engine,
    _server: palmcode_llm::mock::MockServer,
    _rt: tokio::runtime::Runtime,
    base: String,
}

fn setup(scripts: Vec<Vec<u8>>) -> Rig {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let server = rt
        .block_on(serve(MockConfig {
            scripts: scripts
                .into_iter()
                .map(|body| ChatScript {
                    body,
                    ..Default::default()
                })
                .collect(),
            ..Default::default()
        }))
        .unwrap();
    let collector = Arc::new(Collector(Mutex::new(vec![])));
    let dir = tempfile::tempdir().unwrap();
    let engine = Engine::start(
        &dir.path().join("t.db"),
        collector.clone() as Arc<dyn EventSink>,
    )
    .unwrap();
    let base = server.base_url();
    engine.configure(cfg(base.clone())).unwrap();
    Rig {
        _dir: dir,
        c: collector,
        engine,
        _server: server,
        _rt: rt,
        base,
    }
}

fn workspace_with(files: &[(&str, &str)]) -> tempfile::TempDir {
    let ws = tempfile::tempdir().unwrap();
    for (rel, content) in files {
        let p = ws.path().join(rel);
        if let Some(par) = p.parent() {
            std::fs::create_dir_all(par).unwrap();
        }
        std::fs::write(p, content).unwrap();
    }
    ws
}

#[test]
fn read_tool_executes_and_loops_to_answer() {
    let rig = setup(vec![
        fixture("tool_call.sse", TOOL_CALL_READ),
        fixture("tool_result_followup.sse", FOLLOWUP),
    ]);
    let Rig { c, engine, .. } = &rig;
    let ws = workspace_with(&[("src/main.rs", "fn main() { println!(\"hi\"); }\n")]);
    engine.set_workspace(ws.path().to_str().unwrap()).unwrap();

    let sess = engine.create_session("t").unwrap();
    let mid = engine
        .send(&sess.id, "read src/main.rs & summarize")
        .unwrap();
    wait_for(
        c,
        |e| matches!(e, Event::MessageFinished { message_id, .. } if *message_id == mid),
        "MessageFinished",
    );

    assert!(c.has(|e| matches!(
        e,
        Event::ToolStarted { name, path, .. } if name == "read" && path.as_deref() == Some("src/main.rs")
    )));
    assert!(c.has(|e| matches!(e, Event::ToolFinished { ok: true, .. })));
    // reads are allow-by-default: no approval round trip
    assert_eq!(c.count(|e| matches!(e, Event::ApprovalRequested { .. })), 0);
    // tool result fed back; followup text streamed
    let text =
        c.0.lock()
            .unwrap()
            .iter()
            .rev()
            .find_map(|e| match e {
                Event::TextDelta { snapshot, .. } => Some(snapshot.clone()),
                _ => None,
            })
            .unwrap();
    assert!(text.contains("minimal Rust program"), "{text}");
}

/// one content chunk + finish with usage + [DONE]
fn text_with_usage(text: &str, prompt: u32, completion: u32) -> Vec<u8> {
    format!(
        "data: {{\"choices\":[{{\"delta\":{{\"content\":\"{text}\"}}}}]}}\n\n\
         data: {{\"choices\":[{{\"delta\":{{}},\"finish_reason\":\"stop\"}}],\"usage\":{{\"prompt_tokens\":{prompt},\"completion_tokens\":{completion}}}}}\n\n\
         data: [DONE]\n\n"
    )
    .into_bytes()
}

#[test]
fn usage_calibrates_bytes_per_token_ema() {
    let rig = setup(vec![
        text_with_usage("first", 200, 4),
        text_with_usage("second", 400, 4),
    ]);
    let Rig { c, engine, .. } = &rig;
    // no workspace: isolates the message-bytes calibration from schema bytes

    let sess = engine.create_session("t").unwrap();
    let mid = engine.send(&sess.id, "hello").unwrap();
    wait_for(
        c,
        |e| matches!(e, Event::MessageFinished { message_id, .. } if *message_id == mid),
        "MessageFinished",
    );
    engine.shutdown();

    // G: real usage rewrote the default 3.5 bytes/token for this model
    let store = palmcode_store::Store::open(&rig._dir.path().join("t.db")).unwrap();
    let ema: f64 = store
        .meta_get("ema_bpt:mock-glm")
        .unwrap()
        .expect("ema persisted")
        .parse()
        .unwrap();
    assert!(
        (0.5..8.0).contains(&ema) && (ema - 3.5).abs() > 1e-9,
        "ema moved off the default toward observed bytes/token: {ema}"
    );
}

#[test]
fn write_waits_for_approval_then_errors_on_bad_args() {
    // the renamed fixture carries {"path": "...", no content}: proves
    // errors become tool results (loop continues), per deliverable A
    let rig = setup(vec![
        fixture("tool_call.sse", TOOL_CALL_WRITE),
        fixture("tool_result_followup.sse", FOLLOWUP),
    ]);
    let Rig { c, engine, .. } = &rig;
    let ws = workspace_with(&[("src/main.rs", "old contents\n")]);
    engine.set_workspace(ws.path().to_str().unwrap()).unwrap();

    let sess = engine.create_session("t").unwrap();
    let mid = engine.send(&sess.id, "change it").unwrap();
    wait_for(
        c,
        |e| matches!(e, Event::ApprovalRequested { request_id: 1, .. }),
        "ApprovalRequested",
    );
    engine.resolve_approval(1, true, ApprovalScope::Session);
    wait_for(
        c,
        |e| matches!(e, Event::MessageFinished { message_id, .. } if *message_id == mid),
        "MessageFinished",
    );
    // approved + executed, but args invalid -> is_error tool result
    assert!(c.has(|e| matches!(
        e,
        Event::ApprovalResolved {
            request_id: 1,
            approved: true,
            ..
        }
    )));
    assert!(c.has(|e| matches!(
        e,
        Event::ToolFinished { ok: false, preview, .. } if preview.contains("invalid write args")
    )));
}

#[test]
fn denied_write_runs_nothing() {
    let rig = setup(vec![
        fixture("tool_call.sse", TOOL_CALL_WRITE),
        fixture("tool_result_followup.sse", FOLLOWUP),
    ]);
    let Rig { c, engine, .. } = &rig;
    let ws = workspace_with(&[("src/main.rs", "untouchable\n")]);
    engine.set_workspace(ws.path().to_str().unwrap()).unwrap();

    let sess = engine.create_session("t").unwrap();
    let mid = engine.send(&sess.id, "change it").unwrap();
    wait_for(
        c,
        |e| matches!(e, Event::ApprovalRequested { request_id: 1, .. }),
        "ApprovalRequested",
    );
    engine.resolve_approval(1, false, ApprovalScope::Once);
    wait_for(
        c,
        |e| matches!(e, Event::MessageFinished { message_id, .. } if *message_id == mid),
        "MessageFinished",
    );
    assert!(c.has(|e| matches!(
        e,
        Event::ToolFinished { ok: false, preview, .. } if preview.contains("denied by the user")
    )));
    assert_eq!(
        std::fs::read_to_string(ws.path().join("src/main.rs")).unwrap(),
        "untouchable\n"
    );
}

#[test]
fn parallel_reads_all_finish_in_call_order() {
    let rig = setup(vec![
        fixture("parallel_tools.sse", PARALLEL_READS),
        fixture("tool_result_followup.sse", FOLLOWUP),
    ]);
    let Rig { c, engine, .. } = &rig;
    let ws = workspace_with(&[
        ("Cargo.toml", "[package]\nname = \"x\"\n"),
        ("src/lib.rs", "pub fn a() {}\n"),
    ]);
    engine.set_workspace(ws.path().to_str().unwrap()).unwrap();

    let sess = engine.create_session("t").unwrap();
    let mid = engine.send(&sess.id, "both").unwrap();
    wait_for(
        c,
        |e| matches!(e, Event::MessageFinished { message_id, .. } if *message_id == mid),
        "MessageFinished",
    );
    let started: Vec<String> =
        c.0.lock()
            .unwrap()
            .iter()
            .filter_map(|e| match e {
                Event::ToolStarted { call_id, .. } => Some(call_id.clone()),
                _ => None,
            })
            .collect();
    let finished: Vec<String> =
        c.0.lock()
            .unwrap()
            .iter()
            .filter_map(|e| match e {
                Event::ToolFinished {
                    call_id, ok: true, ..
                } => Some(call_id.clone()),
                _ => None,
            })
            .collect();
    assert_eq!(started.len(), 2);
    assert_eq!(finished.len(), 2);
    // fixture order: call-b8de (Cargo.toml read), then call-a4a1 (ls src)
    assert_eq!(started, finished, "results must be assembled in call order");
}

#[test]
fn accept_edits_skips_the_sheet() {
    let rig = setup(vec![
        fixture("tool_call.sse", TOOL_CALL_WRITE),
        fixture("tool_result_followup.sse", FOLLOWUP),
    ]);
    let Rig { c, engine, .. } = &rig;
    let ws = workspace_with(&[("src/main.rs", "data\n")]);
    engine.set_workspace(ws.path().to_str().unwrap()).unwrap();

    let sess = engine.create_session("t").unwrap();
    engine.set_accept_edits(&sess.id, true);
    let mid = engine.send(&sess.id, "go").unwrap();
    wait_for(
        c,
        |e| matches!(e, Event::MessageFinished { message_id, .. } if *message_id == mid),
        "MessageFinished",
    );
    assert_eq!(c.count(|e| matches!(e, Event::ApprovalRequested { .. })), 0);
    assert!(c.has(|e| matches!(e, Event::AutoApproveEdits { on: true, .. })));
}

#[test]
fn kill_mid_tool_never_reruns_on_resume() {
    // (1) run a write call that never completes because the app "dies"
    // between started-row and finish; simulate the crash by dropping the
    // engine then booting a fresh one on the same DB.
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("t.db");
    let ws = workspace_with(&[("src/main.rs", "v1\n")]);

    let _server_rt = tokio::runtime::Runtime::new().unwrap();
    let _server = _server_rt
        .block_on(serve(MockConfig {
            scripts: vec![ChatScript {
                body: fixture("tool_call.sse", TOOL_CALL_WRITE),
                ..Default::default()
            }],
            ..Default::default()
        }))
        .unwrap();
    let c1 = Arc::new(Collector(Mutex::new(vec![])));
    let engine1 = Engine::start(&db, c1.clone() as Arc<dyn EventSink>).unwrap();
    engine1.configure(cfg(_server.base_url())).unwrap();
    engine1.set_workspace(ws.path().to_str().unwrap()).unwrap();
    let sess = engine1.create_session("t").unwrap();
    let mid = engine1.send(&sess.id, "change").unwrap();
    wait_for(
        &c1,
        |e| matches!(e, Event::ApprovalRequested { request_id: 1, .. }),
        "ApprovalRequested",
    );
    // approve -> tool starts -> we kill the engine before finishing the row
    // (drop between approval emission and execution completion; even if the
    // tool already ran, the row stays 'pending' and must never re-run)
    engine1.resolve_approval(1, true, ApprovalScope::Once);
    std::thread::sleep(Duration::from_millis(50));
    drop(engine1);

    let c2 = Arc::new(Collector(Mutex::new(vec![])));
    let engine2 = Engine::start(&db, c2.clone() as Arc<dyn EventSink>).unwrap();
    // boot marks pending tool rows interrupted
    let store = palmcode_store::Store::open(&db).unwrap();
    let calls = store.tool_calls_for(mid).unwrap();
    assert_eq!(calls.len(), 1);
    // invariant: nothing is ever left pending after a reboot (status may
    // legitimately be error/done/interrupted depending on kill timing)
    assert_ne!(calls[0].status, "pending");
    assert!(store.pending_tool_calls().unwrap().is_empty());
    // and never leaves a live pending row
    assert!(engine2.list_sessions().is_ok());
    drop(engine2);
}
