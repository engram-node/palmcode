//! palmcode-bench: deterministic harness-overhead measurement against the
//! mock vLLM server, plus a paced mock server for on-device UI jank runs
//! (`adb reverse tcp:8787` + app base URL http://127.0.0.1:8787/v1).

use std::time::{Duration, Instant};

use palmcode_llm::builder::ChatRequest;
use palmcode_llm::client::{Client, ClientConfig, SecretKey, Timeouts};
use palmcode_llm::mock::{ChatScript, MockConfig, serve};
use palmcode_llm::types::Message;

const LONG_PREFILL: &[u8] = include_bytes!("../../llm/fixtures/vllm/long_prefill.sse");

#[tokio::main(flavor = "multi_thread", worker_threads = 2)]
async fn main() {
    let mode = std::env::args().nth(1).unwrap_or_else(|| "overhead".into());
    match mode.as_str() {
        "overhead" => overhead().await,
        "serve" => {
            let tps: f64 = std::env::args()
                .nth(2)
                .and_then(|a| a.parse().ok())
                .unwrap_or(100.0);
            serve_paced(tps).await
        }
        other => {
            eprintln!("usage: palmcode-bench [overhead|serve <tps>], got {other}");
            std::process::exit(2);
        }
    }
}

/// Harness overhead per turn: N turns over a mock that replays the
/// long_prefill fixture in small chunks with a fixed delay. Measured wall
/// time minus total scripted delay = client-side processing cost
/// (parse + JSON + events). Deterministic to ~ms on a quiet host.
async fn overhead() {
    const CHUNK: usize = 48;
    const DELAY: Duration = Duration::from_millis(2);
    const TURNS: u32 = 25;
    let n_chunks = LONG_PREFILL.len().div_ceil(CHUNK);
    let scripted = DELAY * (n_chunks as u32 - 1);

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
        let t0 = Instant::now();
        client.chat(&req, tx).await.expect("chat");
        if warm >= 3 {
            let wall = t0.elapsed();
            // expected scripted floor: delays + the (unscripted) connect/request write
            let overhead_ms = wall.as_secs_f64() * 1000.0 - scripted.as_secs_f64() * 1000.0;
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

/// Endless paced streamer: each request gets the long_prefill fixture
/// re-chunked to hit the requested tokens/s.
async fn serve_paced(tps: f64) {
    // fixture: glm's mean text chunk ~ 4 chars/token; measure from the file
    let bytes = LONG_PREFILL.len();
    let chunk_bytes = 8_usize;
    let chunks = bytes / chunk_bytes;
    let est_tokens = chunks as f64; // 1 token per chunk by construction
    let delay = Duration::from_secs_f64(1.0 / tps);
    println!(
        "jank server: {bytes} bytes in {chunks}-ish chunks at {tps} tok/s -> {:?}/chunk; stream lasts ~{:.0} s",
        delay,
        est_tokens / tps
    );
    let server = serve(MockConfig {
        scripts: vec![ChatScript {
            body: LONG_PREFILL.to_vec(),
            chunk_size: chunk_bytes,
            inter_chunk_delay: delay,
            hang_after_body: true, // hold stream: stop-button test reuses this
            ..Default::default()
        }],
        tokenize_status: 403,
        ..Default::default()
    })
    .await
    .expect("mock server");
    println!("listening: {}", server.base_url());
    // keep forever
    loop {
        tokio::time::sleep(Duration::from_secs(3600)).await;
    }
}
