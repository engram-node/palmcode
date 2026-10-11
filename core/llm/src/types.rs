//! Wire types for vLLM's OpenAI-compatible API.
//!
//! Request serialization MUST be byte-stable (docs/VLLM.md prefix-stability
//! rule): `serde_json` is built without `preserve_order`, so object keys
//! serialize in sorted order; struct fields serialize in declaration order —
//! neither is allowed to change ad hoc.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    System,
    User,
    Assistant,
    Tool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallSpec {
    pub id: String,
    pub name: String,
    /// Raw JSON arguments text exactly as streamed by the server.
    pub arguments: String,
}

/// A message as sent to / received from the API. `reasoning` is captured
/// for display but NEVER re-sent (vLLM drops it server-side anyway and it
/// would pollute the byte-stable prefix).
#[derive(Debug, Clone, Serialize)]
pub struct Message {
    pub role: Role,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<ApiToolCall>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ApiToolCall {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: &'static str, // always "function"
    pub function: ApiFunctionCall,
}

#[derive(Debug, Clone, Serialize)]
pub struct ApiFunctionCall {
    pub name: String,
    pub arguments: String,
}

impl Message {
    pub fn system(text: impl Into<String>) -> Self {
        Self {
            role: Role::System,
            content: Some(text.into()),
            tool_calls: vec![],
            tool_call_id: None,
        }
    }
    pub fn user(text: impl Into<String>) -> Self {
        Self {
            role: Role::User,
            content: Some(text.into()),
            tool_calls: vec![],
            tool_call_id: None,
        }
    }
    pub fn assistant(text: impl Into<String>) -> Self {
        Self {
            role: Role::Assistant,
            content: Some(text.into()),
            tool_calls: vec![],
            tool_call_id: None,
        }
    }
    pub fn assistant_tool_calls(calls: Vec<ToolCallSpec>, text: Option<String>) -> Self {
        Self {
            role: Role::Assistant,
            content: text,
            tool_calls: calls
                .into_iter()
                .map(|c| ApiToolCall {
                    id: c.id,
                    kind: "function",
                    function: ApiFunctionCall {
                        name: c.name,
                        arguments: c.arguments,
                    },
                })
                .collect(),
            tool_call_id: None,
        }
    }
}

/// Which reasoning field the server streams (probe result).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReasoningField {
    Content, // reasoning_content
    Bare,    // reasoning
}

impl ReasoningField {
    pub fn name(self) -> &'static str {
        match self {
            ReasoningField::Content => "reasoning_content",
            ReasoningField::Bare => "reasoning",
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct ToolCallDelta {
    pub index: Option<u32>,
    pub id: Option<String>,
    pub name: Option<String>,
    pub arguments: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct Usage {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub cached_tokens: Option<u32>,
}

/// One parsed `chat.completion.chunk`.
#[derive(Debug, Clone, Default)]
pub struct Chunk {
    pub text: Option<String>,
    pub reasoning: Option<(ReasoningField, String)>,
    pub tool_deltas: Vec<ToolCallDelta>,
    pub finish_reason: Option<String>,
    pub usage: Option<Usage>,
    /// True when this chunk carried a non-null `usage` object; the probe
    /// uses the count of these to detect `continuous_usage_stats`.
    pub has_usage_object: bool,
}

/// Parse one SSE `data:` payload. Returns `Ok(None)` for `[DONE]`.
pub fn parse_chunk(payload: &str) -> Result<Option<Chunk>, crate::error::LlmError> {
    let payload = payload.trim();
    if payload == "[DONE]" {
        return Ok(None);
    }
    let v: serde_json::Value =
        serde_json::from_str(payload).map_err(|e| crate::error::LlmError::Decode(e.to_string()))?;
    if let Some(err) = v.get("error") {
        // vLLM streams errors mid-response as {"error": {...}} objects.
        return Err(crate::error::LlmError::Decode(format!(
            "server error object: {}",
            err.to_string().chars().take(300).collect::<String>()
        )));
    }
    let mut chunk = Chunk::default();

    if let Some(usage) = v.get("usage").filter(|u| !u.is_null()) {
        chunk.has_usage_object = true;
        let cached = usage
            .get("prompt_tokens_details")
            .and_then(|d| d.get("cached_tokens"))
            .and_then(|t| t.as_u64())
            .map(|t| t as u32);
        chunk.usage = Some(Usage {
            prompt_tokens: usage
                .get("prompt_tokens")
                .and_then(|t| t.as_u64())
                .unwrap_or(0) as u32,
            completion_tokens: usage
                .get("completion_tokens")
                .and_then(|t| t.as_u64())
                .unwrap_or(0) as u32,
            cached_tokens: cached,
        });
    }

    let delta = v
        .get("choices")
        .and_then(|c| c.as_array())
        .and_then(|c| c.first())
        .and_then(|c| c.get("delta"));
    if let Some(delta) = delta {
        if let Some(t) = delta.get("content").and_then(|c| c.as_str())
            && !t.is_empty()
        {
            chunk.text = Some(t.to_string());
        }
        // Reasoning: `reasoning_content` (majority) or bare `reasoning`.
        for (key, field) in [
            ("reasoning_content", ReasoningField::Content),
            ("reasoning", ReasoningField::Bare),
        ] {
            if let Some(t) = delta.get(key).and_then(|c| c.as_str())
                && !t.is_empty()
            {
                chunk.reasoning = Some((field, t.to_string()));
                break;
            }
        }
        if let Some(calls) = delta.get("tool_calls").and_then(|c| c.as_array()) {
            for call in calls {
                chunk.tool_deltas.push(ToolCallDelta {
                    index: call.get("index").and_then(|i| i.as_u64()).map(|i| i as u32),
                    id: call.get("id").and_then(|i| i.as_str()).map(String::from),
                    name: call
                        .get("function")
                        .and_then(|f| f.get("name"))
                        .and_then(|n| n.as_str())
                        .map(String::from),
                    arguments: call
                        .get("function")
                        .and_then(|f| f.get("arguments"))
                        .and_then(|a| a.as_str())
                        .map(String::from),
                });
            }
        }
    }
    if let Some(fr) = v
        .get("choices")
        .and_then(|c| c.as_array())
        .and_then(|c| c.first())
        .and_then(|c| c.get("finish_reason"))
        .and_then(|f| f.as_str())
    {
        chunk.finish_reason = Some(fr.to_string());
    }

    Ok(Some(chunk))
}

#[derive(Debug, Clone)]
pub struct ModelInfo {
    pub id: String,
    pub max_model_len: Option<u32>,
}

/// `GET /models` response (vLLM may or may not expose max_model_len).
pub fn parse_models(body: &serde_json::Value) -> Vec<ModelInfo> {
    body.get("data")
        .and_then(|d| d.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|m| {
                    Some(ModelInfo {
                        id: m.get("id")?.as_str()?.to_string(),
                        max_model_len: m
                            .get("max_model_len")
                            .and_then(|l| l.as_u64())
                            .map(|l| l as u32),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn message_serialization_is_byte_stable() {
        let mk = || {
            vec![
                Message::system("sys"),
                Message::user("hello"),
                Message::assistant("hi there"),
                Message::assistant_tool_calls(
                    vec![ToolCallSpec {
                        id: "c1".into(),
                        name: "read".into(),
                        arguments: "{\"path\":\"/a\"}".into(),
                    }],
                    None,
                ),
            ]
        };
        let a = serde_json::to_string(&mk()).unwrap();
        let b = serde_json::to_string(&mk()).unwrap();
        assert_eq!(a, b);
        // no reasoning field may ever be serialized
        assert!(!a.contains("reasoning"));
    }

    #[test]
    fn cached_tokens_parsed_from_details() {
        let payload = r#"{"choices":[{"delta":{},"finish_reason":"stop"}],
            "usage":{"prompt_tokens":100,"completion_tokens":10,
            "prompt_tokens_details":{"cached_tokens":80}}}"#;
        let c = parse_chunk(payload).unwrap().unwrap();
        let u = c.usage.unwrap();
        assert_eq!(u.cached_tokens, Some(80));
        assert_eq!(c.finish_reason.as_deref(), Some("stop"));
    }

    #[test]
    fn both_reasoning_fields_accepted() {
        let a = parse_chunk(r#"{"choices":[{"delta":{"reasoning_content":"think"}}]}"#)
            .unwrap()
            .unwrap();
        assert_eq!(a.reasoning.unwrap().0.name(), "reasoning_content");
        let b = parse_chunk(r#"{"choices":[{"delta":{"reasoning":"think"}}]}"#)
            .unwrap()
            .unwrap();
        assert_eq!(b.reasoning.unwrap().0.name(), "reasoning");
    }
}
