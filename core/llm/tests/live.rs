//! Manual real-endpoint tests. Never run in CI: both env vars must be set.
//! Keys come from the environment only and are never committed.
//!
//!   PALMCODE_LLM_API_KEY=sk-... PALMCODE_LLM_BASE_URL=https://host/v1 \
//!       cargo test -p palmcode-llm --test live -- --nocapture

use std::time::Duration;

use palmcode_llm::builder::ChatRequest;
use palmcode_llm::client::{Client, ClientConfig, SecretKey, Timeouts};
use palmcode_llm::types::Message;

fn live_client() -> Option<(Client, String)> {
    let key = std::env::var("PALMCODE_LLM_API_KEY").ok()?;
    let base = std::env::var("PALMCODE_LLM_BASE_URL")
        .unwrap_or_else(|_| "https://grid.ai.juspay.net/v1".into());
    let client = Client::new(ClientConfig {
        base_url: base,
        api_key: SecretKey::new(key),
        timeouts: Timeouts::default(),
        continuous_usage: false,
    })
    .ok()?;
    Some((client, "glm-latest".into()))
}

#[tokio::test]
async fn live_probe_and_chat() {
    let Some((client, model)) = live_client() else {
        eprintln!("env not set — skipping live test");
        return;
    };
    let report = palmcode_llm::probe::probe(&client, &model).await;
    eprintln!("probe: {report:#?}");
    assert!(
        report.models.ok,
        "models section failed: {:?}",
        report.models
    );

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let collect = tokio::spawn(async move {
        let mut text = String::new();
        while let Some(ev) = rx.recv().await {
            if let palmcode_llm::client::ClientEvent::Text(t) = ev {
                text.push_str(&t);
            }
        }
        text
    });
    let out = client
        .chat(
            &ChatRequest {
                model,
                messages: vec![Message::user("Reply with exactly: ok")],
                max_tokens: 512,
                continuous_usage: false,
                sampling: None,
                extra_body: None,
                tools: None,
            },
            tx,
        )
        .await
        .expect("live chat");
    let text = collect.await.unwrap();
    eprintln!(
        "turn: finish={} ttft={:?} usage={:?} text={text:?}",
        out.finish_reason, out.ttft, out.usage
    );
    assert!(out.ttft.unwrap_or_default() < Duration::from_secs(120));
    assert!(!text.is_empty());
}
