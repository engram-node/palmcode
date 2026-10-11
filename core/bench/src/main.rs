//! palmcode-bench: deterministic harness-overhead measurement against the
//! mock vLLM server, plus a paced mock server for on-device UI jank runs
//! (`adb reverse tcp:8787` + app base URL http://127.0.0.1:8787/v1).

use std::time::{Duration, Instant};

use palmcode_llm::builder::ChatRequest;
use palmcode_llm::client::{Client, ClientConfig, SecretKey, Timeouts};
use palmcode_llm::mock::{ChatScript, MockConfig, serve, serve_on};
use palmcode_llm::types::Message;

const LONG_PREFILL: &[u8] = include_bytes!("../../llm/fixtures/vllm/long_prefill.sse");

#[tokio::main(flavor = "multi_thread", worker_threads = 2)]
async fn main() {
    let mode = std::env::args().nth(1).unwrap_or_else(|| "overhead".into());
    match mode.as_str() {
        "overhead" => overhead().await,
        "serve" => {
            let arg = |i: usize| std::env::args().nth(i);
            let tps: f64 = arg(2).and_then(|a| a.parse().ok()).unwrap_or(100.0);
            let secs: f64 = arg(3).and_then(|a| a.parse().ok()).unwrap_or(60.0);
            let port: u16 = arg(4).and_then(|a| a.parse().ok()).unwrap_or(8787);
            serve_paced(tps, secs, port).await
        }
        other => {
            eprintln!("usage: palmcode-bench [overhead|serve <tps>], got {other}");
            std::process::exit(2);
        }
    }
}

/// Harness overhead per turn: N turns over a mock that replays the
/// long_prefill fixture in small chunks with a fixed delay. Measured wall
/// time minus the mock's OWN measured stream span = client-side processing
/// cost (connect + request write + parse + JSON + events). The scripted
/// delay is not subtracted: tokio's timer granularity (~15 ms on Windows)
/// makes it worthless on coarse-timer hosts.
async fn overhead() {
    const CHUNK: usize = 48;
    const DELAY: Duration = Duration::from_millis(2);
    const TURNS: u32 = 25;
    let n_chunks = LONG_PREFILL.len().div_ceil(CHUNK);

    let server = serve(MockConfig {
        scripts: vec![ChatScript {
            body: LONG_PREFILL.to_vec(),
            chunk_size: CHUNK,
            inter_chunk_delay: DELAY,
            ..Default::default()
        }],
        ..Default::default()
    })
    .await
    .expect("mock server");

    let client = Client::new(ClientConfig {
        base_url: server.base_url(),
        api_key: SecretKey::new("sk-bench".into()),
        timeouts: Timeouts::default(),
        continuous_usage: false,
    })
    .expect("client");
    let req = ChatRequest {
        model: "mock-glm".into(),
        messages: vec![Message::user("bench")],
        max_tokens: 4096,
        continuous_usage: false,
        sampling: None,
        extra_body: None,
        tools: None,
    };

    let mut overheads = Vec::new();
    // warmup
    for warm in 0..=TURNS + 3 {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let measured_before = server.stream_spans.lock().unwrap().len();
        let t0 = Instant::now();
        client.chat(&req, tx).await.expect("chat");
        if warm >= 3 {
            let wall = t0.elapsed();
            let server_span = server
                .stream_spans
                .lock()
                .unwrap()
                .drain(measured_before..)
                .next()
                .unwrap_or_default();
            let overhead_ms = (wall.as_secs_f64() - server_span.as_secs_f64()) * 1000.0;
            overheads.push(overhead_ms.max(0.0));
        }
    }
    overheads.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let p50 = overheads[overheads.len() / 2];
    let p95 = overheads[(overheads.len() as f64 * 0.95) as usize - 1];
    println!(
        "overhead per turn (excl. scripted waits, incl. parse+event pipeline):\n  p50 {p50:.1} ms\n  p95 {p95:.1} ms\n  chunks/turn {n_chunks}\n  budget < 50 ms: {}",
        if p95 < 50.0 { "PASS" } else { "FAIL" }
    );
    std::process::exit(if p95 < 50.0 { 0 } else { 1 });
}

/// Endless paced streamer: synthetic chunks at the requested tok/s on a
/// FIXED port (adb reverse tcp maps it). One TCP write per chunk.
async fn serve_paced(tps: f64, secs: f64, port: u16) {
    let mut body = String::new();
    let n = (secs * tps) as u64;
    for _ in 0..n {
        // one ~8-byte token per chunk, valid JSON, no escaping worries
        body.push_str("data: {\"choices\":[{\"delta\":{\"content\":\"toktok  \"}}]}\n\n");
    }
    body.push_str("data: [DONE]\n\n");
    let body = body.into_bytes();
    let delay = Duration::from_secs_f64(1.0 / tps);
    println!(
        "jank server: {} chunks ({} bytes) at {tps} tok/s -> {:?}/chunk; stream lasts ~{secs:.0} s",
        n,
        body.len(),
        delay,
    );
    let listener = std::net::TcpListener::bind(("127.0.0.1", port)).expect("bind");
    listener.set_nonblocking(true).expect("nonblocking");
    let server = serve_on(
        tokio::net::TcpListener::from_std(listener).expect("tokio listener"),
        MockConfig {
            scripts: vec![ChatScript {
                body,
                chunk_size: 60, // one SSE line per write
                inter_chunk_delay: delay,
                hang_after_body: true, // hold stream: stop-button test reuses this
                ..Default::default()
            }],
            tokenize_status: 403,
            ..Default::default()
        },
    )
    .await
    .expect("mock server");
    println!("listening: {}", server.base_url());
    // keep forever; surface client disconnects (stop-button proof)
    loop {
        if server
            .disconnected
            .swap(false, std::sync::atomic::Ordering::SeqCst)
        {
            println!("client-disconnected observed");
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}
