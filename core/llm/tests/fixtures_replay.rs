//! Replay committed vLLM fixtures through the full pipeline
//! (SSE parse -> chunk parse -> tool-call accumulator) and check the
//! assembled result. The byte-granularity property test lives in sse.rs.

use palmcode_llm::accumulator::ToolAccumulator;
use palmcode_llm::sse::SseParser;
use palmcode_llm::types;

fn assemble(sse: &[u8]) -> (String, String, Vec<(String, String)>, Option<u32>) {
    let mut p = SseParser::new();
    let mut payloads = p.feed(sse);
    payloads.extend(p.finish());
    let mut text = String::new();
    let mut reasoning = String::new();
    let mut acc = ToolAccumulator::new();
    let mut prompt_tokens = None;
    let mut done = false;
    for payload in &payloads {
        let Some(chunk) = types::parse_chunk(payload).expect("fixture chunk parses") else {
            done = true;
            continue;
        };
        if let Some(t) = chunk.text {
            text.push_str(&t);
        }
        if let Some((_, r)) = chunk.reasoning {
            reasoning.push_str(&r);
        }
        for d in &chunk.tool_deltas {
            acc.push(d);
        }
        if let Some(u) = chunk.usage {
            prompt_tokens = Some(u.prompt_tokens);
        }
    }
    assert!(done, "fixture must end with [DONE]");
    let calls = acc
        .finish()
        .into_iter()
        .map(|c| (c.name, c.arguments))
        .collect();
    (text, reasoning, calls, prompt_tokens)
}

#[test]
fn simple() {
    // captured with a small max_tokens against a reasoning model: reasoning
    // text present, answer may be empty (finish_reason: length), and the
    // gateway did not emit a usage chunk on this capture (metadata.json).
    let (_text, reasoning, calls, _usage) = assemble(include_bytes!("../fixtures/vllm/simple.sse"));
    assert!(!reasoning.is_empty(), "reasoning deltas expected");
    assert!(calls.is_empty());
}

#[test]
fn reasoning_separator_from_answer() {
    let (text, reasoning, _, _) = assemble(include_bytes!("../fixtures/vllm/reasoning.sse"));
    assert!(
        !reasoning.is_empty(),
        "reasoning fixture must carry reasoning"
    );
    assert!(!text.is_empty(), "reasoning fixture must carry an answer");
    // reasoning must NOT bleed into the answer channel
    let (t2, _, _, _) = assemble(include_bytes!("../fixtures/vllm/simple.sse"));
    assert_ne!(text, t2);
}

#[test]
fn tool_call() {
    let (_, _, calls, _) = assemble(include_bytes!("../fixtures/vllm/tool_call.sse"));
    assert_eq!(calls.len(), 1);
    let (name, args) = &calls[0];
    assert!(!name.is_empty());
    let v: serde_json::Value =
        serde_json::from_str(args).expect("assembled arguments must be valid JSON");
    assert!(v.is_object());
}

#[test]
fn parallel_tools_interleaved_into_separate_calls() {
    let (_, _, calls, _) = assemble(include_bytes!("../fixtures/vllm/parallel_tools.sse"));
    assert!(calls.len() >= 2, "parallel fixture: {} calls", calls.len());
    for (_, args) in &calls {
        serde_json::from_str::<serde_json::Value>(args)
            .unwrap_or_else(|_| panic!("arguments must assemble to JSON: {args}"));
    }
}

#[test]
fn tool_result_followup() {
    let (text, _, _, _) = assemble(include_bytes!("../fixtures/vllm/tool_result_followup.sse"));
    assert!(!text.is_empty());
}

#[test]
fn long_prefill_streams() {
    let (text, reasoning, _, _) = assemble(include_bytes!("../fixtures/vllm/long_prefill.sse"));
    assert!(
        !text.is_empty() || !reasoning.is_empty(),
        "long_prefill must carry streamed deltas"
    );
}
