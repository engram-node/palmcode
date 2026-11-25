//! Process-kill recovery, using a real host bash call and a local mock only.
use super::*;
use palmcode_llm::mock::{ChatScript, MockConfig, serve};
use std::process::{Child, Command, Stdio};

#[derive(Default)]
struct Collector(Mutex<Vec<Event>>);
impl EventSink for Collector {
    fn on_batch(&self, events: Vec<Event>) {
        self.0.lock().unwrap().extend(events);
    }
}

fn config(base_url: String) -> ProviderCfg {
    ProviderCfg {
        base_url,
        api_key: "test-only".into(),
        main_model: "test".into(),
        fast_model: "test".into(),
        context_window: 128_000,
        max_output_tokens: 512,
        sampling_json: None,
        extra_body_json: None,
        continuous_usage: false,
        first_token_secs: 5,
        idle_secs: 5,
    }
}

fn wait_until(mut ready: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(15);
    while !ready() {
        assert!(Instant::now() < deadline, "crash test condition timed out");
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
#[ignore = "subprocess helper; invoked by killed_bash_is_not_replayed"]
fn engine_child() {
    let root = PathBuf::from(std::env::var_os("PALMCODE_TEST_CRASH_ROOT").unwrap());
    let command = "echo started >> run-count; exec >&- 2>&-; while [ ! -f release ]; do sleep 0.05; done; echo finished > child-finished";
    let chunk = serde_json::json!({"choices": [{"index": 0, "delta": {"role": "assistant", "tool_calls": [{
        "index": 0, "id": "crash-bash", "type": "function", "function": {"name": "bash",
        "arguments": serde_json::json!({"command": command}).to_string()}
    }]}, "finish_reason": "tool_calls"}]});
    let rt = tokio::runtime::Runtime::new().unwrap();
    let server = rt
        .block_on(serve(MockConfig {
            scripts: vec![ChatScript {
                body: format!("data: {chunk}\n\ndata: [DONE]\n\n").into_bytes(),
                ..Default::default()
            }],
            ..Default::default()
        }))
        .unwrap();
    let collector = Arc::new(Collector::default());
    let engine = Engine::start(&root.join("engine.db"), collector.clone()).unwrap();
    engine.configure(config(server.base_url())).unwrap();
    engine.set_workspace(root.to_str().unwrap()).unwrap();
    let session = engine.create_session("crash regression").unwrap();
    std::fs::write(root.join("session"), &session.id).unwrap();
    engine
        .send(&session.id, "run the supplied test command")
        .unwrap();
    loop {
        let approvals: Vec<_> = collector
            .0
            .lock()
            .unwrap()
            .iter()
            .filter_map(|e| match e {
                Event::ApprovalRequested { request_id, .. } => Some(*request_id),
                _ => None,
            })
            .collect();
        for id in approvals {
            engine.resolve_approval(id, true, ApprovalScope::Once);
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

struct ChildGuard {
    child: Child,
    root: PathBuf,
}
impl Drop for ChildGuard {
    fn drop(&mut self) {
        // Release an orphan on hosts without Linux PDEATHSIG/subreaping too.
        let _ = std::fs::write(self.root.join("release"), "release");
        let _ = self.child.kill();
        let _ = self.child.wait();
        // Keep the directory until the Windows orphan observes the release.
        #[cfg(windows)]
        {
            let deadline = Instant::now() + Duration::from_secs(2);
            while !self.root.join("child-finished").exists() && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(20));
            }
        }
    }
}

#[test]
fn killed_bash_is_not_replayed() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let child = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "crash_tests::engine_child",
            "--ignored",
            "--test-threads=1",
        ])
        .env_clear()
        .env("PALMCODE_TEST_CRASH_ROOT", root)
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .env(
            "SystemRoot",
            std::env::var_os("SystemRoot").unwrap_or_default(),
        )
        .env("TEMP", root)
        .env("TMP", root)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut guard = ChildGuard {
        child,
        root: root.to_owned(),
    };
    wait_until(|| root.join("run-count").exists());
    let session = std::fs::read_to_string(root.join("session")).unwrap();
    let store = Store::open(&root.join("engine.db")).unwrap();
    assert_eq!(store.pending_tool_calls().unwrap().len(), 1);
    guard.child.kill().unwrap(); // Kill the engine process while bash is running.
    guard.child.wait().unwrap();
    let collector = Arc::new(Collector::default());
    let engine = Engine::start(&root.join("engine.db"), collector.clone()).unwrap();
    assert!(store.pending_tool_calls().unwrap().is_empty());
    let history = engine.api_history(&session).unwrap();
    assert!(history.iter().any(|m| {
        m.tool_call_id.as_deref() == Some("crash-bash")
            && m.content
                .as_deref()
                .is_some_and(|s| s.contains("interrupted, outcome unknown"))
    }));
    let rt = tokio::runtime::Runtime::new().unwrap();
    let server = rt.block_on(serve(MockConfig {
        scripts: vec![ChatScript { body: b"data: {\"choices\":[{\"delta\":{\"content\":\"Recovered\"},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n".to_vec(), ..Default::default() }],
        ..Default::default()
    })).unwrap();
    engine.configure(config(server.base_url())).unwrap();
    engine.set_workspace(root.to_str().unwrap()).unwrap();
    engine.resume(&session).unwrap();
    wait_until(|| {
        collector
            .0
            .lock()
            .unwrap()
            .iter()
            .any(|e| matches!(e, Event::MessageFinished { .. }))
    });
    assert_eq!(
        std::fs::read_to_string(root.join("run-count"))
            .unwrap()
            .lines()
            .count(),
        1
    );
    assert!(
        !collector
            .0
            .lock()
            .unwrap()
            .iter()
            .any(|e| matches!(e, Event::ToolStarted { .. }))
    );
    let rows = store.transcript(&session).unwrap();
    let calls: Vec<_> = rows
        .iter()
        .flat_map(|(m, _)| store.tool_calls_for(m.id).unwrap())
        .collect();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].status, "interrupted");
}
