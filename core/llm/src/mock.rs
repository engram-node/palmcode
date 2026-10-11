//! Hand-rolled mock vLLM server on a raw TCP listener: scripted SSE
//! responses with explicit delays, plus observable client disconnect.
//! Used by unit tests, the bench harness, and (over `adb reverse`) the UI
//! jank run. No runtime HTTP server dependency.

use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

/// What to stream for `POST /v1/chat/completions`.
#[derive(Clone)]
pub struct ChatScript {
    pub status: u16,
    pub retry_after: Option<u64>,
    /// Raw bytes of the SSE body (fixtures pass through unmodified).
    pub body: Vec<u8>,
    /// Re-chunk the body into pieces of this size (0 = one write).
    pub chunk_size: usize,
    /// Delay between writes (token pacing).
    pub inter_chunk_delay: Duration,
    /// Delay before the FIRST byte (first-token simulation).
    pub first_byte_delay: Duration,
    /// Send headers, write `body`, then hold the connection open until the
    /// client disconnects (no [DONE]).
    pub hang_after_body: bool,
}

impl Default for ChatScript {
    fn default() -> Self {
        Self {
            status: 200,
            retry_after: None,
            body: b"data: {\"choices\":[{\"delta\":{\"content\":\"ok\"},\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":3,\"completion_tokens\":1}}\n\ndata: [DONE]\n\n".to_vec(),
            chunk_size: 0,
            inter_chunk_delay: Duration::ZERO,
            first_byte_delay: Duration::ZERO,
            hang_after_body: false,
        }
    }
}

pub struct MockServer {
    pub port: u16,
    /// The client hung up while a stream was open (cancellation proof).
    pub disconnected: Arc<AtomicBool>,
    /// How many requests hit the server (retry counting).
    pub hits: Arc<AtomicU32>,
    /// Bodies of received requests (assertion material).
    pub bodies: Arc<Mutex<Vec<Vec<u8>>>>,
    shutdown: Option<tokio::sync::oneshot::Sender<()>>,
}

impl MockServer {
    pub fn base_url(&self) -> String {
        format!("http://127.0.0.1:{}/v1", self.port)
    }
}

/// Response selection per request: `scripts` is a queue — hit n consumes
/// entry min(n, len-1). Extra routes (models/tokenize/version) are canned
/// unless `models_body`/`tokenize_status` override them.
pub struct MockConfig {
    pub scripts: Vec<ChatScript>,
    pub models_body: Option<Vec<u8>>,
    pub tokenize_status: u16,
    pub tokenize_count: u32,
}

impl Default for MockConfig {
    fn default() -> Self {
        Self {
            scripts: vec![ChatScript::default()],
            models_body: None,
            tokenize_status: 200,
            tokenize_count: 41,
        }
    }
}

pub async fn serve(cfg: MockConfig) -> std::io::Result<MockServer> {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await?;
    let port = listener.local_addr()?.port();
    let disconnected = Arc::new(AtomicBool::new(false));
    let hits = Arc::new(AtomicU32::new(0));
    let bodies = Arc::new(Mutex::new(Vec::<Vec<u8>>::new()));
    let (stop_tx, mut stop_rx) = tokio::sync::oneshot::channel::<()>();

    {
        let (disconnected, hits, bodies) = (disconnected.clone(), hits.clone(), bodies.clone());
        // scripts are consumed per CHAT REQUEST (not per connection:
        // /models and /tokenize must not advance the queue)
        let scripts = Arc::new(Mutex::new(cfg.scripts.into_iter().collect::<Vec<_>>()));
        tokio::spawn(async move {
            loop {
                let (mut sock, _) = tokio::select! {
                    _ = &mut stop_rx => break,
                    a = listener.accept() => match a { Ok(v) => v, Err(_) => break },
                };
                let (disconnected, hits, bodies, scripts) = (
                    disconnected.clone(),
                    hits.clone(),
                    bodies.clone(),
                    scripts.clone(),
                );
                let cfg_models = cfg.models_body.clone();
                let (tok_status, tok_count) = (cfg.tokenize_status, cfg.tokenize_count);
                tokio::spawn(async move {
                    let _ = handle(
                        &mut sock,
                        scripts,
                        cfg_models,
                        tok_status,
                        tok_count,
                        hits,
                        bodies,
                        disconnected,
                    )
                    .await;
                });
            }
        });
    }
    Ok(MockServer {
        port,
        disconnected,
        hits,
        bodies,
        shutdown: Some(stop_tx),
    })
}

impl Drop for MockServer {
    fn drop(&mut self) {
        if let Some(tx) = self.shutdown.take() {
            let _ = tx.send(());
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn handle(
    sock: &mut tokio::net::TcpStream,
    scripts: Arc<Mutex<Vec<ChatScript>>>,
    models_body: Option<Vec<u8>>,
    tokenize_status: u16,
    tokenize_count: u32,
    hits: Arc<AtomicU32>,
    bodies: Arc<Mutex<Vec<Vec<u8>>>>,
    disconnected: Arc<AtomicBool>,
) -> std::io::Result<()> {
    hits.fetch_add(1, Ordering::SeqCst);
    // --- read request (headers + content-length body) ---
    let mut buf = vec![0u8; 32 * 1024];
    let mut filled = 0usize;
    let mut header_end = None;
    while filled < buf.len() {
        let n = sock.read(&mut buf[filled..]).await?;
        if n == 0 {
            disconnected.store(true, Ordering::SeqCst);
            return Ok(());
        }
        filled += n;
        if let Some(i) = find_subslice(&buf[..filled], b"\r\n\r\n") {
            header_end = Some(i);
            break;
        }
    }
    let header_end = match header_end {
        Some(i) => i,
        None => return Ok(()),
    };
    let header = String::from_utf8_lossy(&buf[..header_end]).to_string();
    let content_length: usize = header
        .lines()
        .find_map(|l| {
            let (k, v) = l.split_once(':')?;
            if k.trim().eq_ignore_ascii_case("content-length") {
                v.trim().parse::<usize>().ok()
            } else {
                None
            }
        })
        .unwrap_or(0);
    let mut body = buf[header_end + 4..filled].to_vec();
    while body.len() < content_length {
        let n = sock.read(&mut buf).await?;
        if n == 0 {
            break;
        }
        body.extend_from_slice(&buf[..n]);
    }
    let request_line = header.lines().next().unwrap_or_default().to_string();
    bodies.lock().unwrap().push(body);
    // scripts are consumed per CHAT request only (/models, /tokenize and
    // retries of a failed chat must not silently advance the queue)
    let is_chat = request_line.starts_with("POST /v1/chat/completions");
    let script = if is_chat {
        let mut q = scripts.lock().unwrap();
        if q.len() > 1 {
            q.remove(0)
        } else {
            q.first().cloned().unwrap_or_default()
        }
    } else {
        ChatScript::default()
    };

    // --- route ---
    let (status, extra, payload): (u16, Vec<u8>, Vec<u8>) = if request_line
        .starts_with("GET /v1/models")
    {
        (
                200,
                vec![],
                models_body.unwrap_or_else(|| {
                    br#"{"data":[{"id":"mock-glm","max_model_len":128000},{"id":"mock-glm-flash"}],"object":"list"}"#
                        .to_vec()
                }),
            )
    } else if request_line.starts_with("POST /v1/tokenize") {
        (
            tokenize_status,
            vec![],
            format!("{{\"count\":{tokenize_count},\"max_model_len\":128000,\"tokens\":[1,2]}}")
                .into_bytes(),
        )
    } else if request_line.starts_with("POST /v1/chat/completions") {
        (
            script.status,
            script
                .retry_after
                .map(|s| format!("retry-after: {s}\r\n").into_bytes())
                .unwrap_or_default(),
            // for non-200 chat responses `body` doubles as the error payload
            if script.status == 200 {
                vec![]
            } else {
                script.body.clone()
            },
        )
    } else {
        (404, vec![], b"not found".to_vec())
    };
    if status != 200 || !request_line.starts_with("POST /v1/chat/completions") {
        let mut head = format!(
            "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n",
            payload.len()
        )
        .into_bytes();
        head.extend_from_slice(&extra);
        head.extend_from_slice(b"\r\n");
        sock.write_all(&head).await?;
        sock.write_all(&payload).await?;
        sock.shutdown().await?;
        return Ok(());
    }

    // --- streaming SSE ---
    let mut head = b"HTTP/1.1 200\r\ncontent-type: text/event-stream\r\ncache-control: no-cache\r\nconnection: close\r\n".to_vec();
    head.extend_from_slice(&extra);
    head.extend_from_slice(b"\r\n");
    sock.write_all(&head).await?;

    if script.first_byte_delay > Duration::ZERO {
        tokio::time::sleep(script.first_byte_delay).await;
    }
    let pieces: Vec<&[u8]> = if script.chunk_size == 0 {
        vec![&script.body]
    } else {
        script.body.chunks(script.chunk_size).collect()
    };
    for p in pieces {
        if script.inter_chunk_delay > Duration::ZERO {
            tokio::time::sleep(script.inter_chunk_delay).await;
        }
        if sock.write_all(p).await.is_err() {
            disconnected.store(true, Ordering::SeqCst);
            return Ok(());
        }
    }
    if script.hang_after_body {
        // hold the stream open: client cancellation must be visible here
        let mut one = [0u8; 1];
        for _ in 0..2000u32 {
            match sock.try_read(&mut one) {
                Ok(0) => {
                    disconnected.store(true, Ordering::SeqCst);
                    return Ok(());
                }
                Ok(_) => {}
                Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(_) => {
                    disconnected.store(true, Ordering::SeqCst);
                    return Ok(());
                }
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }
    sock.shutdown().await?;
    Ok(())
}

fn find_subslice(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}
