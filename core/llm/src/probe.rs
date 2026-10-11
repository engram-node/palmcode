//! vLLM-aware endpoint probe (in-app equivalent of scripts/probe-vllm.ps1).
//! Each section is isolated: one failure never masks the others. Capability
//! findings prefill the provider config; every one is user-overridable.

use std::time::Duration;

use tokio::sync::mpsc;

use crate::builder::ChatRequest;
use crate::client::{Cleartext, Client};
use crate::error::LlmError;
use crate::types::{Message, ModelInfo};

pub const TOOL_REMEDIATION: &str =
    "server missing --enable-auto-tool-choice / --tool-call-parser <model-parser> (docs/VLLM.md)";

#[derive(Debug)]
pub struct ProbeSection {
    pub ok: bool,
    pub detail: String,
}

impl ProbeSection {
    fn ok(detail: impl Into<String>) -> Self {
        Self {
            ok: true,
            detail: detail.into(),
        }
    }
    fn fail(detail: impl Into<String>) -> Self {
        Self {
            ok: false,
            detail: detail.into(),
        }
    }
}

#[derive(Debug)]
pub struct ProbeReport {
    pub cleartext: Cleartext,
    pub models: ProbeSection,
    pub model_ids: Vec<String>,
    /// largest max_model_len across models (prefills context window)
    pub max_model_len: Option<u32>,
    pub tokenize: ProbeSection,
    pub tool_choice: ProbeSection,
    /// "reasoning_content" | "reasoning" | none observed
    pub reasoning_field: Option<&'static str>,
    /// server emitted usage on more than one chunk
    pub continuous_usage: bool,
    /// usage.prompt_tokens_details.cached_tokens present anywhere
    pub cached_tokens_exposed: bool,
    pub ttft: Option<Duration>,
    pub sample_tps: Option<f64>,
    /// the chat probe request itself failed (401 etc.)
    pub chat: Option<String>,
}

pub async fn probe(client: &Client, model: &str) -> ProbeReport {
    let mut report = ProbeReport {
        cleartext: client.cleartext(),
        models: ProbeSection::fail("not run"),
        model_ids: vec![],
        max_model_len: None,
        tokenize: ProbeSection::fail("not run"),
        tool_choice: ProbeSection::fail("not run"),
        reasoning_field: None,
        continuous_usage: false,
        cached_tokens_exposed: false,
        ttft: None,
        sample_tps: None,
        chat: None,
    };

    // 1. /models
    match client.models().await {
        Ok(ms) => {
            report.max_model_len = ms.iter().filter_map(|m| m.max_model_len).max();
            let with_len = ms.iter().filter(|m| m.max_model_len.is_some()).count();
            report.models = ProbeSection::ok(format!(
                "{} model(s), {} with max_model_len",
                ms.len(),
                with_len
            ));
            report.model_ids = ms.into_iter().map(|m: ModelInfo| m.id).collect();
        }
        Err(e) => {
            report.models = ProbeSection::fail(format!("GET /models failed: {e}"));
            // chat would fail the same way (likely auth/network); still run
            // the tool probe so the panel never shows a blank column
            report.tool_choice = probe_tool_choice(client, model).await;
            return report;
        }
    }

    // 2. /tokenize
    report.tokenize = match client.tokenize(model, "ping").await {
        Ok(n) => ProbeSection::ok(format!("reachable ({n} tokens for \"ping\")")),
        Err(LlmError::Http { status, .. }) => {
            ProbeSection::fail(format!("HTTP {status} — heuristic fallback (chars/3.5)"))
        }
        Err(e) => ProbeSection::fail(format!("unreachable ({e}) — heuristic fallback")),
    };

    // 3. tiny streaming chat: ttft, reasoning field, usage flavor, rate
    let (tx, mut rx) = mpsc::unbounded_channel();
    // drain events so sends never block; ttft comes from the outcome
    let drain = tokio::spawn(async move { while rx.recv().await.is_some() {} });
    match client
        .chat(
            &ChatRequest {
                model: model.to_string(),
                messages: vec![Message::user("Reply with exactly: ok")],
                max_tokens: 512, // reasoning models need headroom (docs/VLLM.md)
                continuous_usage: client.continuous_usage(),
                sampling: None,
                extra_body: None,
                tools: None,
            },
            tx,
        )
        .await
    {
        Ok(outcome) => {
            report.ttft = outcome.ttft;
            report.reasoning_field = outcome.reasoning_field.map(|f| f.name());
            report.continuous_usage = outcome.usage_chunks > 1;
            if let Some(u) = &outcome.usage {
                if u.cached_tokens.is_some() {
                    report.cached_tokens_exposed = true;
                }
                let decode_secs =
                    outcome.elapsed.as_secs_f64() - outcome.ttft.unwrap_or_default().as_secs_f64();
                if decode_secs > 0.0 && u.completion_tokens > 0 {
                    report.sample_tps = Some(u.completion_tokens as f64 / decode_secs);
                }
            }
        }
        Err(e) => report.chat = Some(format!("{e}")),
    }
    let _ = drain.await;

    // 4. tool_choice: "auto" detection
    report.tool_choice = probe_tool_choice(client, model).await;
    report
}

async fn probe_tool_choice(client: &Client, model: &str) -> ProbeSection {
    let (tx, _rx) = mpsc::unbounded_channel();
    let tool = serde_json::json!({
        "type": "function",
        "function": {
            "name": "noop",
            "description": "probe-only tool; never called",
            "parameters": {"type": "object", "properties": {}}
        }
    });
    let res = client
        .chat(
            &ChatRequest {
                model: model.to_string(),
                messages: vec![Message::user("Do not use any tool. Reply: ok")],
                max_tokens: 1,
                continuous_usage: client.continuous_usage(),
                sampling: None,
                extra_body: None,
                tools: Some(vec![tool]),
            },
            tx,
        )
        .await;
    match res {
        Ok(_) => ProbeSection::ok("tool_choice=\"auto\" accepted"),
        Err(LlmError::ToolChoiceRejected { detail }) => ProbeSection::fail(format!(
            "{TOOL_REMEDIATION} — server said: {}",
            detail.chars().take(200).collect::<String>()
        )),
        Err(e) => ProbeSection::fail(format!("unexpected: {e}")),
    }
}
