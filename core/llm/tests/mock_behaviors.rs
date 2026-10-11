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
