//! Behavior tests against the hand-rolled mock server (docs/VLLM.md
//! "Timeouts & aborts"): retry before first token, Retry-After honored,
//! idle-stall timeout, cancellation = server-observed disconnect.

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use palmcode_llm::builder::ChatRequest;
use palmcode_llm::client::{Client, ClientConfig, SecretKey, Timeouts};
use palmcode_llm::error::LlmError;
use palmcode_llm::mock::{ChatScript, MockConfig, serve};
use palmcode_llm::types::Message;

const OK_SSE: &[u8] = b"data: {\"choices\":[{\"delta\":{\"content\":\"hi\"}}]}\n\ndata: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":3,\"completion_tokens\":1}}\n\ndata: [DONE]\n\n";

#[tokio::test]
async fn unterminated_final_sse_event_delivers_the_response() {
    let body = b"data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0}]}}]}\n\ndata: {\"choices\":[{\"delta\":{\"content\":\"Here is the answer\"},\"finish_reason\":\"stop\"}],\"usage\":{\"completion_tokens\":14}}".to_vec();
    let server = serve(MockConfig {
        scripts: vec![ChatScript {
            body,
            ..Default::default()
        }],
        ..Default::default()
    })
    .await
    .unwrap();
    let c = client(&server, timeouts(2000, 2000));
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    c.chat(&req(), tx).await.unwrap();
    let mut text = String::new();
    while let Ok(event) = rx.try_recv() {
        if let palmcode_llm::client::ClientEvent::Text(delta) = event {
            text.push_str(&delta);
        }
    }
    assert_eq!(text, "Here is the answer");
}

#[tokio::test]
async fn done_ends_the_response_without_waiting_for_socket_close() {
    let server = serve(MockConfig {
        scripts: vec![ChatScript {
            body: OK_SSE.to_vec(),
            hang_after_body: true,
            ..Default::default()
        }],
        ..Default::default()
    })
    .await
    .unwrap();
    let c = client(&server, timeouts(2000, 100));
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    // The server deliberately keeps the response open. Waiting for EOF would
    // hit the idle timeout even though the provider already sent [DONE].
    c.chat(&req(), tx).await.unwrap();
}

#[tokio::test]
async fn nameless_tool_stub_is_an_error_without_retry() {
    for name in ["", "   "] {
        let payload = serde_json::json!({"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"name":name,"arguments":"{}"}}]},"finish_reason":"stop"}],"usage":{"completion_tokens":14}});
        let body = format!("data: {payload}\n\ndata: [DONE]\n\n").into_bytes();
        let server = serve(MockConfig {
            scripts: vec![ChatScript {
                body,
                ..Default::default()
            }],
            ..Default::default()
        })
        .await
        .unwrap();
        let c = client(&server, timeouts(2000, 2000));
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        assert!(
            matches!(c.chat(&req(), tx).await, Err(LlmError::Decode(message)) if message.contains("incomplete tool call"))
        );
        assert_eq!(server.hits.load(Ordering::SeqCst), 1);
    }
}

#[tokio::test]
async fn tool_stub_followed_by_final_unterminated_name_is_a_valid_call() {
    let body = b"data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call_1\"}]}}]}\n\ndata: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"name\":\"read\",\"arguments\":\"{}\"}}]},\"finish_reason\":\"tool_calls\"}]}".to_vec();
    let server = serve(MockConfig {
        scripts: vec![ChatScript {
            body,
            ..Default::default()
        }],
        ..Default::default()
    })
    .await
    .unwrap();
    let c = client(&server, timeouts(2000, 2000));
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    c.chat(&req(), tx).await.unwrap();
    let mut acc = palmcode_llm::accumulator::ToolAccumulator::new();
    while let Ok(event) = rx.try_recv() {
        if let palmcode_llm::client::ClientEvent::Tools(deltas) = event {
            for delta in deltas {
                acc.push(&delta);
            }
        }
    }
    let calls = acc.finish();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].name, "read");
}

#[tokio::test]
async fn full_message_and_text_parts_reach_client_text_events() {
    for choice in [
        serde_json::json!({"message":{"content":"Hello there"},"finish_reason":"stop"}),
        serde_json::json!({"delta":{"content":[{"type":"text","text":"Hello "},{"type":"text","text":"there"}]},"finish_reason":"stop"}),
    ] {
        let body = format!(
            "data: {}\n\ndata: [DONE]\n\n",
            serde_json::json!({"choices":[choice]})
        )
        .into_bytes();
        let server = serve(MockConfig {
            scripts: vec![ChatScript {
                body,
                ..Default::default()
            }],
            ..Default::default()
        })
        .await
        .unwrap();
        let c = client(&server, timeouts(2000, 2000));
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        c.chat(&req(), tx).await.unwrap();
        let mut text = String::new();
        while let Ok(event) = rx.try_recv() {
            if let palmcode_llm::client::ClientEvent::Text(delta) = event {
                text.push_str(&delta);
            }
        }
        assert_eq!(text, "Hello there");
    }
}

#[tokio::test]
async fn final_full_message_does_not_duplicate_streamed_text() {
    let body = b"data: {\"choices\":[{\"delta\":{\"content\":\"Hello\"}}]}\n\ndata: {\"choices\":[{\"message\":{\"content\":\"Hello\"},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n".to_vec();
    let server = serve(MockConfig {
        scripts: vec![ChatScript {
            body,
            ..Default::default()
        }],
        ..Default::default()
    })
    .await
    .unwrap();
    let c = client(&server, timeouts(2000, 2000));
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    c.chat(&req(), tx).await.unwrap();
    let mut text = String::new();
    while let Ok(event) = rx.try_recv() {
        if let palmcode_llm::client::ClientEvent::Text(delta) = event {
            text.push_str(&delta);
        }
    }
    assert_eq!(text, "Hello");
}

#[tokio::test]
async fn final_full_message_completes_a_partial_delta() {
    let body = b"data: {\"choices\":[{\"delta\":{\"content\":\"Hello\"}}]}\n\ndata: {\"choices\":[{\"delta\":{},\"message\":{\"content\":\"Hello there\"},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n".to_vec();
    let server = serve(MockConfig {
        scripts: vec![ChatScript {
            body,
            ..Default::default()
        }],
        ..Default::default()
    })
    .await
    .unwrap();
    let c = client(&server, timeouts(2000, 2000));
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    c.chat(&req(), tx).await.unwrap();
    let mut text = String::new();
    while let Ok(event) = rx.try_recv() {
        if let palmcode_llm::client::ClientEvent::Text(delta) = event {
            text.push_str(&delta);
        }
    }
    assert_eq!(text, "Hello there");
}

#[tokio::test]
async fn usage_only_response_is_an_explicit_error_without_retry_or_first_token() {
    let body = b"data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}],\"usage\":{\"completion_tokens\":34}}\n\ndata: [DONE]\n\n".to_vec();
    let server = serve(MockConfig {
        scripts: vec![ChatScript {
            body,
            ..Default::default()
        }],
        ..Default::default()
    })
    .await
    .unwrap();
    let c = client(&server, timeouts(2000, 2000));
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    assert!(
        matches!(c.chat(&req(), tx).await, Err(LlmError::Decode(message)) if message.contains("no response text"))
    );
    assert_eq!(server.hits.load(Ordering::SeqCst), 1);
    while let Ok(event) = rx.try_recv() {
        assert!(!matches!(
            event,
            palmcode_llm::client::ClientEvent::FirstToken { .. }
        ));
    }
}

fn timeouts(first_ms: u64, idle_ms: u64) -> Timeouts {
    Timeouts {
        first_token: Duration::from_millis(first_ms),
        idle: Duration::from_millis(idle_ms),
    }
}

fn client(server: &palmcode_llm::mock::MockServer, t: Timeouts) -> Client {
    Client::new(ClientConfig {
        base_url: server.base_url(),
        api_key: SecretKey::new("sk-test-mock-key".into()),
        timeouts: t,
        continuous_usage: false,
    })
    .expect("client")
}

fn req() -> ChatRequest {
    ChatRequest {
        model: "mock-glm".into(),
        messages: vec![Message::user("hi")],
        max_tokens: 16,
        continuous_usage: false,
        sampling: None,
        extra_body: None,
        tools: None,
    }
}

#[tokio::test]
async fn happy_path_via_mock() {
    let server = serve(MockConfig::default()).await.unwrap();
    let c = client(&server, timeouts(1000, 1000));
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let out = c.chat(&req(), tx).await.unwrap();
    assert_eq!(out.finish_reason, "stop");
    assert_eq!(out.usage.unwrap().prompt_tokens, 3);
    assert!(out.ttft.is_some());
    assert_eq!(server.hits.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn standard_tool_chat_wire_omits_disabled_continuous_usage_extension() {
    let server = serve(MockConfig::default()).await.unwrap();
    let c = client(&server, timeouts(1000, 1000));
    let mut request = req();
    request.tools = Some(vec![serde_json::json!({
        "name": "read",
        "description": "Read a file",
        "input_schema": {"type": "object", "properties": {}},
    })]);
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    c.chat(&request, tx).await.unwrap();
    let bodies = server.bodies.lock().unwrap();
    assert_eq!(bodies.len(), 1);
    let wire: serde_json::Value = serde_json::from_slice(&bodies[0]).unwrap();
    assert_eq!(
        wire["stream_options"],
        serde_json::json!({"include_usage": true})
    );
    assert_eq!(wire["tool_choice"], "auto");
    assert!(
        wire["tools"]
            .as_array()
            .is_some_and(|tools| !tools.is_empty())
    );
}

#[tokio::test]
async fn probe_does_not_enable_unrequested_extension_from_usage_chunks() {
    let usage_stream = b"data: {\"choices\":[{\"delta\":{\"content\":\"ok\"}}],\"usage\":{\"prompt_tokens\":3,\"completion_tokens\":1}}\n\ndata: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":3,\"completion_tokens\":1}}\n\ndata: [DONE]\n\n";
    for enabled in [false, true] {
        let server = serve(MockConfig {
            scripts: vec![ChatScript {
                body: usage_stream.to_vec(),
                ..Default::default()
            }],
            ..Default::default()
        })
        .await
        .unwrap();
        let c = Client::new(ClientConfig {
            base_url: server.base_url(),
            api_key: SecretKey::new("synthetic-fixture".into()),
            timeouts: timeouts(1000, 1000),
            continuous_usage: enabled,
        })
        .unwrap();
        let report = palmcode_llm::probe::probe(&c, "fixture").await;
        assert!(report.chat.is_none());
        assert_eq!(report.continuous_usage, enabled);
    }
}

#[tokio::test]
async fn retries_before_first_token_then_succeeds() {
    let server = serve(MockConfig {
        scripts: vec![
            ChatScript {
                status: 500,
                ..Default::default()
            },
            ChatScript {
                status: 200,
                body: OK_SSE.to_vec(),
                ..Default::default()
            },
        ],
        ..Default::default()
    })
    .await
    .unwrap();
    let c = client(&server, timeouts(1000, 1000));
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let out = c.chat(&req(), tx).await.unwrap();
    assert_eq!(out.retries, 1);
    assert_eq!(server.hits.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn retry_after_is_honored_and_capped() {
    let server = serve(MockConfig {
        scripts: vec![
            ChatScript {
                status: 429,
                retry_after: Some(1),
                ..Default::default()
            },
            ChatScript::default(),
        ],
        ..Default::default()
    })
    .await
    .unwrap();
    let c = client(&server, timeouts(1000, 1000));
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let started = Instant::now();
    c.chat(&req(), tx).await.unwrap();
    assert!(
        started.elapsed() >= Duration::from_millis(900),
        "429 Retry-After: 1 must delay the retry"
    );
}

#[tokio::test]
async fn idle_stall_aborts() {
    let server = serve(MockConfig {
        scripts: vec![ChatScript {
            body: b"data: {\"choices\":[{\"delta\":{\"content\":\"hi\"}}]}\n\n".to_vec(),
            hang_after_body: true,
            ..Default::default()
        }],
        ..Default::default()
    })
    .await
    .unwrap();
    let c = client(&server, timeouts(2000, 400));
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let started = Instant::now();
    let err = c.chat(&req(), tx).await.unwrap_err();
    assert!(
        matches!(err, LlmError::IdleStall(0) | LlmError::IdleStall(1)),
        "{err}"
    );
    assert!(started.elapsed() < Duration::from_secs(3));
}

#[tokio::test]
async fn first_token_timeout_aborts() {
    let server = serve(MockConfig {
        scripts: vec![ChatScript {
            first_byte_delay: Duration::from_millis(800),
            ..Default::default()
        }],
        ..Default::default()
    })
    .await
    .unwrap();
    let c = client(&server, timeouts(200, 2000));
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let err = c.chat(&req(), tx).await.unwrap_err();
    assert!(matches!(err, LlmError::FirstTokenTimeout(0)), "{err}");
}

/// THE M1 gate (docs/VLLM.md): user abort must close the TCP connection;
/// the server side observes it.
#[tokio::test]
async fn mock_server_observes_client_disconnect() {
    let server = serve(MockConfig {
        scripts: vec![ChatScript {
            body: b"data: {\"choices\":[{\"delta\":{\"content\":\"chunk \"}}]}\n\n".to_vec(),
            hang_after_body: true,
            ..Default::default()
        }],
        ..Default::default()
    })
    .await
    .unwrap();
    let c = client(&server, timeouts(5000, 60_000));
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let handle = tokio::spawn({
        let c = c.clone();
        async move { c.chat(&req(), tx).await }
    });
    // let the first chunk through, then cancel
    tokio::time::sleep(Duration::from_millis(200)).await;
    handle.abort();
    let _ = handle.await;
    // server must observe the disconnect (its try_read sees EOF)
    for _ in 0..100 {
        if server.disconnected.load(Ordering::SeqCst) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("server never saw the client disconnect");
}

#[tokio::test]
async fn unauthorized_and_tool_400_map() {
    let server = serve(MockConfig {
        scripts: vec![ChatScript {
            status: 401,
            ..Default::default()
        }],
        ..Default::default()
    })
    .await
    .unwrap();
    let c = client(&server, timeouts(1000, 1000));
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let err = c.chat(&req(), tx).await.unwrap_err();
    assert!(matches!(err, LlmError::Http { status: 401, .. }), "{err}");

    // tool_choice 400 with a vLLM-style body -> actionable ToolChoiceRejected
    let mut tool_req = req();
    tool_req.tools = Some(vec![
        serde_json::json!({"type": "function", "function": {"name": "x", "parameters": {}}}),
    ]);
    let vllm_400 = br#"{"error":{"message":"auto tool choice requires --enable-auto-tool-choice and --tool-call-parser","type":"BadRequestError"}}"#;
    let server2 = serve(MockConfig {
        scripts: vec![ChatScript {
            status: 400,
            body: vllm_400.to_vec(),
            ..Default::default()
        }],
        ..Default::default()
    })
    .await
    .unwrap();
    let c2 = client(&server2, timeouts(1000, 1000));
    let err = c2
        .chat(&tool_req, tokio::sync::mpsc::unbounded_channel().0)
        .await
        .unwrap_err();
    match err {
        LlmError::ToolChoiceRejected { detail } => {
            assert!(detail.contains("tool-call-parser"), "{detail}");
        }
        other => panic!("expected ToolChoiceRejected, got {other}"),
    }
}

/// The client's user data must not reach logs: tracing capture.
#[tokio::test]
async fn api_key_absent_from_logs_and_errors() {
    let mut log_capture = TestLogCapture::install();
    let server = serve(MockConfig {
        scripts: vec![ChatScript {
            status: 401,
            ..Default::default()
        }],
        ..Default::default()
    })
    .await
    .unwrap();
    let key = "sk-never-log-me-0123456789";
    let c = Client::new(ClientConfig {
        base_url: server.base_url(),
        api_key: SecretKey::new(key.into()),
        timeouts: timeouts(1000, 1000),
        continuous_usage: false,
    })
    .unwrap();
    let err = c
        .chat(&req(), tokio::sync::mpsc::unbounded_channel().0)
        .await
        .unwrap_err();
    let captured = log_capture.drain();
    assert!(!captured.contains(key), "logs leaked key: {captured}");
    assert!(!format!("{err:?}").contains(key));
    assert!(!format!("{err}").contains(key));
    assert!(!format!("{c:?}").contains(key));
    // also: the mock saw the request — header arrived but we never logged it
    assert_eq!(server.hits.load(Ordering::SeqCst), 1);
}

// tracing subscriber capturing into a buffer (test-local, no dep beyond
// tracing-subscriber which is dev-only).
struct TestLogCapture {
    buf: Arc<std::sync::Mutex<String>>,
}
impl TestLogCapture {
    fn install() -> Self {
        let buf = Arc::new(std::sync::Mutex::new(String::new()));
        let w = BufferWriter(buf.clone());
        let sub = tracing_subscriber::fmt()
            .with_writer(move || w.clone())
            .with_ansi(false)
            .with_max_level(tracing::Level::DEBUG)
            .finish();
        let _ = tracing::subscriber::set_global_default(sub); // once per process OK
        Self { buf }
    }
    fn drain(&mut self) -> String {
        std::mem::take(&mut *self.buf.lock().unwrap())
    }
}
#[derive(Clone)]
struct BufferWriter(Arc<std::sync::Mutex<String>>);
impl std::io::Write for BufferWriter {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().push_str(&String::from_utf8_lossy(b));
        Ok(b.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
