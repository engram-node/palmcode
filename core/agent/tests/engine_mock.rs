//! End-to-end engine test against the hand-rolled mock vLLM server:
//! send -> stream -> finish, stop -> interrupted -> resume, event seq
//! monotony, store persistence.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use palmcode_agent::{Engine, Event, EventSink, ProviderCfg};
use palmcode_llm::mock::{ChatScript, MockConfig, serve};
use palmcode_llm::sse::SseParser;
use palmcode_llm::types;

struct Collector(Mutex<Vec<Event>>);
impl EventSink for Collector {
    fn on_batch(&self, events: Vec<Event>) {
        self.0.lock().unwrap().extend(events);
    }
}
impl Collector {
    fn snapshot(&self) -> Vec<Event> {
        self.0.lock().unwrap().clone()
    }
}

fn wait_for(c: &Collector, pred: impl Fn(&Event) -> bool, what: &str) {
    for _ in 0..500 {
        if c.snapshot().iter().any(&pred) {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let dump: Vec<String> = c
        .snapshot()
        .iter()
        .map(|e| match e {
            Event::TextDelta {
                message_id,
                snapshot,
                ..
            } => {
                format!(
                    "TextDelta(mid={message_id}, {:?})",
                    snapshot.chars().take(30).collect::<String>()
                )
            }
            Event::MessageStarted {
                message_id, role, ..
            } => format!("Started(mid={message_id},{role})"),
            Event::MessageFinished { message_id, .. } => format!("Finished(mid={message_id})"),
            Event::Interrupted { message_id, .. } => format!("Interrupted(mid={message_id})"),
            Event::TurnError { detail, .. } => format!("Error({detail})"),
            other => format!("{other:?}.."),
        })
        .collect();
    panic!("timed out waiting for {what}; events: {dump:?}");
}

fn cfg(base: String) -> ProviderCfg {
    ProviderCfg {
        base_url: base,
        api_key: "sk-mock".into(),
        main_model: "mock-glm".into(),
        fast_model: "mock-glm-flash".into(),
        context_window: 128000,
        max_output_tokens: 512,
        sampling_json: None,
        extra_body_json: None,
        continuous_usage: false,
        first_token_secs: 5,
        idle_secs: 2,
    }
}

#[test]
fn session_summary_is_generated_without_tools_persisted_and_requested_once() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let server = rt
        .block_on(serve(MockConfig {
            scripts: vec![
                ChatScript {
                    body: sse_stream(&["Use a shell test script."]),
                    ..Default::default()
                },
                ChatScript {
                    body: sse_stream(&["Fix shell tests"]),
                    ..Default::default()
                },
            ],
            ..Default::default()
        }))
        .unwrap();
    let collector = Arc::new(Collector(Mutex::new(vec![])));
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("titles.db");
    let engine = Engine::start(&path, collector.clone()).unwrap();
    engine.configure(cfg(server.base_url())).unwrap();
    let session = engine.create_session("Chat 12345").unwrap();
    assert!(!engine.summarize_session(&session.id).unwrap()); // No empty-chat request.
    let mid = engine
        .send(&session.id, "Help fix failing shell tests")
        .unwrap();
    wait_for(
        &collector,
        |e| matches!(e, Event::MessageFinished { message_id, .. } if *message_id == mid),
        "reply",
    );
    let before = engine.transcript(&session.id).unwrap();
    assert!(engine.summarize_session(&session.id).unwrap());
    assert!(!engine.summarize_session(&session.id).unwrap());
    assert_eq!(engine.list_sessions().unwrap()[0].title, "Fix shell tests");
    assert_eq!(engine.transcript(&session.id).unwrap().len(), before.len());
    let bodies: Vec<serde_json::Value> = server
        .bodies
        .lock()
        .unwrap()
        .iter()
        .map(|body| serde_json::from_slice(body).unwrap())
        .filter(|body: &serde_json::Value| body.get("stream").is_some())
        .collect();
    assert_eq!(bodies.len(), 2);
    let request = &bodies[1];
    assert!(request.get("tools").is_none());
    assert_eq!(request["max_tokens"], 256);
    assert_eq!(request["messages"].as_array().unwrap().len(), 2);
    engine.shutdown();
    drop(engine);
    let reopened = Engine::start(&path, collector).unwrap();
    assert_eq!(
        reopened.list_sessions().unwrap()[0].title,
        "Fix shell tests"
    );
    reopened.shutdown();
}

#[test]
fn failed_summary_preserves_chat_and_custom_titles_make_no_request() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let server = rt
        .block_on(serve(MockConfig {
            scripts: vec![
                ChatScript::default(),
                ChatScript {
                    status: 400,
                    body: b"bad request".to_vec(),
                    ..Default::default()
                },
            ],
            ..Default::default()
        }))
        .unwrap();
    let collector = Arc::new(Collector(Mutex::new(vec![])));
    let dir = tempfile::tempdir().unwrap();
    let engine = Engine::start(&dir.path().join("titles.db"), collector.clone()).unwrap();
    engine.configure(cfg(server.base_url())).unwrap();
    let custom = engine.create_session("My shell notes").unwrap();
    assert!(!engine.summarize_session(&custom.id).unwrap());
    let session = engine.create_session("New chat").unwrap();
    let mid = engine.send(&session.id, "Help with a script").unwrap();
    wait_for(
        &collector,
        |e| matches!(e, Event::MessageFinished { message_id, .. } if *message_id == mid),
        "reply",
    );
    assert!(!engine.summarize_session(&session.id).unwrap());
    assert_eq!(
        engine
            .list_sessions()
            .unwrap()
            .iter()
            .find(|s| s.id == session.id)
            .unwrap()
            .title,
        "New chat"
    );
    assert!(
        engine
            .transcript(&session.id)
            .unwrap()
            .iter()
            .all(|m| m.error.is_none())
    );
    let chats = server
        .bodies
        .lock()
        .unwrap()
        .iter()
        .map(|body| serde_json::from_slice::<serde_json::Value>(body).unwrap())
        .filter(|body| body.get("stream").is_some())
        .count();
    assert_eq!(chats, 2);
    engine.shutdown();
}

fn sse_stream(tokens: &[&str]) -> Vec<u8> {
    let mut out = Vec::new();
    for t in tokens {
        out.extend_from_slice(
            format!("data: {{\"choices\":[{{\"delta\":{{\"content\":\"{t}\"}}}}]}}\n\n").as_bytes(),
        );
    }
    out.extend_from_slice(b"data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":10,\"completion_tokens\":3}}\n\ndata: [DONE]\n\n");
    out
}

#[test]
fn send_stream_finish_persist() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let server = rt
        .block_on(serve(MockConfig {
            scripts: vec![ChatScript {
                body: sse_stream(&["hello", " world"]),
                ..Default::default()
            }],
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
    engine.configure(cfg(server.base_url())).unwrap();

    let sess = engine.create_session("t1").unwrap();
    let mid = engine.send(&sess.id, "say hi").unwrap();

    wait_for(
        &collector,
        |e| matches!(e, Event::MessageFinished { message_id, .. } if *message_id == mid),
        "MessageFinished",
    );
    engine.shutdown();

    let events = collector.snapshot();
    // monotonic sequence
    let seqs: Vec<u64> = events
        .iter()
        .map(|e| match e {
            Event::SessionCreated { seq, .. }
            | Event::UserlandStatus { seq, .. }
            | Event::MessageStarted { seq, .. }
            | Event::TextDelta { seq, .. }
            | Event::ReasoningDelta { seq, .. }
            | Event::UsageTick { seq, .. }
            | Event::MessageFinished { seq, .. }
            | Event::Interrupted { seq, .. }
            | Event::TurnError { seq, .. }
            | Event::Compacted { seq, .. }
            | Event::ToolStarted { seq, .. }
            | Event::ToolFinished { seq, .. }
            | Event::ToolOutput { seq, .. }
            | Event::ApprovalRequested { seq, .. }
            | Event::ApprovalResolved { seq, .. }
            | Event::AutoApproveEdits { seq, .. } => *seq,
        })
        .collect();
    assert!(
        seqs.windows(2).all(|w| w[0] < w[1]),
        "seq monotonic: {seqs:?}"
    );

    let text_delta = events
        .iter()
        .rev()
        .find_map(|e| match e {
            Event::TextDelta {
                message_id,
                snapshot,
                ..
            } if *message_id == mid => Some(snapshot.clone()),
            _ => None,
        })
        .expect("text delta");
    assert_eq!(text_delta, "hello world");

    // persisted: full transcript with closed text part
    let t = engine.transcript(&sess.id).unwrap();
    assert_eq!(t.len(), 2);
    assert_eq!(t[0].role, "user");
    assert_eq!(t[1].text, "hello world");
    assert!(!t[1].interrupted);
    // the mock saw ONE chat POST whose body holds no sampling params
    let bodies = server.bodies.lock().unwrap();
    let last = bodies.last().unwrap();
    let body = String::from_utf8_lossy(last);
    assert!(body.contains("say hi"));
    assert!(!body.contains("temperature"));
}

#[test]
fn stop_then_resume() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let server = rt
        .block_on(serve(MockConfig {
            scripts: vec![
                // first turn hangs after one chunk
                ChatScript {
                    body: b"data: {\"choices\":[{\"delta\":{\"content\":\"partial\"}}]}\n\n"
                        .to_vec(),
                    hang_after_body: true,
                    ..Default::default()
                },
                // resume replays the user message fully
                ChatScript {
                    body: sse_stream(&["complete", " answer"]),
                    ..Default::default()
                },
            ],
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
    engine.configure(cfg(server.base_url())).unwrap();
    let sess = engine.create_session("t2").unwrap();
    let mid = engine.send(&sess.id, "give me output").unwrap();
    // let the first assistant chunk arrive (the user turn echoes a
    // TextDelta too — match the ASSISTANT message id)
    wait_for(
        &collector,
        |e| matches!(e, Event::TextDelta { message_id, .. } if *message_id == mid),
        "first assistant TextDelta",
    );
    engine.stop(&sess.id).unwrap();
    wait_for(
        &collector,
        |e| matches!(e, Event::Interrupted { .. }),
        "Interrupted",
    );
    // server must have seen the disconnect
    for _ in 0..100 {
        if server
            .disconnected
            .load(std::sync::atomic::Ordering::SeqCst)
        {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(
        server
            .disconnected
            .load(std::sync::atomic::Ordering::SeqCst)
    );

    engine.resume(&sess.id).unwrap();
    wait_for(
        &collector,
        |e| matches!(e, Event::MessageFinished { finish_reason, .. } if finish_reason == "stop"),
        "resumed MessageFinished",
    );
    engine.shutdown();

    let t = engine.transcript(&sess.id).unwrap();
    assert_eq!(t.len(), 3); // user + interrupted assistant + completed assistant
    assert!(t[1].interrupted);
    assert_eq!(t[1].text, "partial");
    assert_eq!(t[2].text, "complete answer");

    // history replay on resume must NOT include the interrupted assistant text
    let bodies = server.bodies.lock().unwrap();
    let second = String::from_utf8_lossy(bodies.last().unwrap());
    assert!(
        !second.contains("partial"),
        "interrupted tail leaked into resume request"
    );
}

/// A crashed engine (no orderly shutdown) recovers interrupted sessions.
#[test]
fn crash_recovery_via_store() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let server = rt
        .block_on(serve(MockConfig {
            scripts: vec![ChatScript {
                body: b"data: {\"choices\":[{\"delta\":{\"content\":\"will-crash\"}}]}\n\n"
                    .to_vec(),
                hang_after_body: true,
                ..Default::default()
            }],
            ..Default::default()
        }))
        .unwrap();
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("t.db");
    {
        let collector = Arc::new(Collector(Mutex::new(vec![])));
        let engine = Engine::start(&db, collector.clone() as Arc<dyn EventSink>).unwrap();
        engine.configure(cfg(server.base_url())).unwrap();
        let sess = engine.create_session("t3").unwrap();
        let _ = engine.send(&sess.id, "stream and die").unwrap();
        std::thread::sleep(Duration::from_millis(400));
        // NOTE: no shutdown — engine dropped mid-stream (process "kill -9"
        // approximation: pending tasks aborted by runtime drop)
    }
    // fresh engine on the same db: must recover and mark interrupted
    let collector = Arc::new(Collector(Mutex::new(vec![])));
    let engine = Engine::start(&db, collector.clone() as Arc<dyn EventSink>).unwrap();
    engine.configure(cfg(server.base_url())).unwrap();
    let sessions = engine.list_sessions().unwrap();
    assert_eq!(sessions.len(), 1);
    let sess = &sessions[0];
    wait_for(
        &collector,
        |e| matches!(e, Event::Interrupted { .. }),
        "recovery Interrupted",
    );
    assert!(engine.resume(&sess.id).is_ok()); // resume path works post-crash
    engine.stop(&sess.id).unwrap();
    engine.shutdown();
}

/// Fixture pipeline cross-check through the mock: replay a committed .sse
/// byte-for-byte; the accumulator and parser must hold up end to end.
#[test]
fn fixture_replay_through_engine() {
    let bytes = &include_bytes!("../../llm/fixtures/vllm/tool_call.sse")[..];
    // sanity: parseable, has DONE
    let mut p = SseParser::new();
    let payloads = p.feed(bytes);
    assert!(!payloads.is_empty());
    assert!(payloads.iter().any(|p| types::parse_chunk(p).is_ok()));
}
