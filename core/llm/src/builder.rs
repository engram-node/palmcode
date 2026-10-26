//! Request construction (docs/VLLM.md "Request shape"):
//! - no sampling params unless the user set them;
//! - deterministic byte output (serde_json sorted object keys);
//! - `extra_body` nested under its own key; sampling overrides merged
//!   top-level (whitelisted keys only).

use crate::error::LlmError;
use crate::types::Message;

/// Sampling keys we allow the user to override. Anything else in the
/// overrides JSON is an error (typos silently diverging = prefix-cache pain).
const SAMPLING_KEYS: &[&str] = &[
    "temperature",
    "top_p",
    "top_k",
    "min_p",
    "repetition_penalty",
    "seed",
];

#[derive(Debug, Clone)]
pub struct ChatRequest {
    pub model: String,
    pub messages: Vec<Message>,
    pub max_tokens: u32,
    /// Server supports `--enable-continuous-usage-stats` (probe result,
    /// user-overridable).
    pub continuous_usage: bool,
    /// User sampling overrides; empty/None = send none.
    pub sampling: Option<serde_json::Value>,
    /// User extra_body (e.g. `{"chat_template_kwargs": {"enable_thinking": false}}`).
    pub extra_body: Option<serde_json::Value>,
    /// Tool definitions (M1: always None; the accumulator is fixture-tested
    /// regardless).
    pub tools: Option<Vec<serde_json::Value>>,
}

pub fn build_body(req: &ChatRequest) -> Result<serde_json::Value, LlmError> {
    let mut body = serde_json::json!({
        "model": req.model,
        "messages": req.messages,
        "max_tokens": req.max_tokens,
        "stream": true,
        "stream_options": {
            "include_usage": true,
            "continuous_usage_stats": req.continuous_usage,
        },
    });
    let obj = body.as_object_mut().expect("json! object");

    if let Some(tools) = &req.tools
        && !tools.is_empty()
    {
        // Schemas arrive provider-neutral ({name, description, input_schema});
        // the OpenAI/vLLM wire wants nested function objects.
        let wire: Vec<serde_json::Value> = tools
            .iter()
            .map(|t| {
                let name = t.get("name").cloned().unwrap_or_default();
                let description = t.get("description").cloned().unwrap_or_default();
                let params = t
                    .get("input_schema")
                    .or_else(|| t.get("parameters"))
                    .cloned()
                    .unwrap_or_else(|| serde_json::json!({"type": "object"}));
                serde_json::json!({
                    "type": "function",
                    "function": {
                        "name": name,
                        "description": description,
                        "parameters": params,
                    }
                })
            })
            .collect();
        obj.insert("tools".into(), serde_json::Value::Array(wire));
        obj.insert(
            "tool_choice".into(),
            serde_json::Value::String("auto".into()),
        );
    }
    if let Some(sampling) = &req.sampling {
        let map = sampling
            .as_object()
            .ok_or_else(|| LlmError::BadJson("sampling overrides must be a JSON object".into()))?;
        for (k, v) in map {
            if !SAMPLING_KEYS.contains(&k.as_str()) {
                return Err(LlmError::BadJson(format!(
                    "sampling override {k:?} not whitelisted ({SAMPLING_KEYS:?})"
                )));
            }
            obj.insert(k.clone(), v.clone());
        }
    }
    if let Some(extra) = &req.extra_body {
        let map = extra
            .as_object()
            .ok_or_else(|| LlmError::BadJson("extra_body must be a JSON object".into()))?;
        if !map.is_empty() {
            obj.insert("extra_body".into(), serde_json::Value::Object(map.clone()));
        }
    }
    Ok(body)
}

pub fn body_bytes(req: &ChatRequest) -> Result<Vec<u8>, LlmError> {
    let body = {
        let _phase = palmcode_telemetry::Phase::start("request_build");
        build_body(req)?
    };
    let _phase = palmcode_telemetry::Phase::start("serialize");
    serde_json::to_vec(&body).map_err(|e| LlmError::BadJson(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::ToolCallSpec;

    #[test]
    fn default_request_sends_no_sampling_params() {
        let req = ChatRequest {
            model: "test-model".into(),
            messages: vec![Message::system("s"), Message::user("u")],
            max_tokens: 4096,
            continuous_usage: false,
            sampling: None,
            extra_body: None,
            tools: None,
        };
        let s = String::from_utf8(body_bytes(&req).unwrap()).unwrap();
        for forbidden in [
            "temperature",
            "top_p",
            "top_k",
            "repetition_penalty",
            "seed",
            "extra_body",
            "tools",
        ] {
            assert!(!s.contains(forbidden), "default body contains {forbidden}");
        }
        assert!(s.contains(r#""stream":true"#));
        assert!(s.contains(r#""include_usage":true"#));
        assert!(s.contains(r#""continuous_usage_stats":false"#));
    }

    #[test]
    fn tools_go_out_openai_shaped() {
        let req = ChatRequest {
            model: "m".into(),
            messages: vec![Message::user("u")],
            max_tokens: 1024,
            continuous_usage: false,
            sampling: None,
            extra_body: None,
            tools: Some(vec![serde_json::json!({
                "name": "read",
                "description": "Read a file",
                "input_schema": {"type": "object", "properties": {"path": {"type": "string"}}}
            })]),
        };
        let body = build_body(&req).unwrap();
        let t = &body["tools"][0];
        assert_eq!(t["type"], "function");
        assert_eq!(t["function"]["name"], "read");
        assert_eq!(t["function"]["description"], "Read a file");
        assert!(t["function"]["parameters"]["properties"]["path"].is_object());
        assert!(
            t.get("input_schema").is_none(),
            "no anthropic key on the wire"
        );
        assert_eq!(body["tool_choice"], "auto");
    }

    #[test]
    fn byte_stable_and_sorted() {
        let req = ChatRequest {
            model: "m".into(),
            messages: vec![
                Message::system("s"),
                Message::user("u"),
                Message::assistant_tool_calls(
                    vec![ToolCallSpec {
                        id: "i".into(),
                        name: "n".into(),
                        arguments: "{}".into(),
                    }],
                    Some("t".into()),
                ),
            ],
            max_tokens: 100,
            continuous_usage: true,
            sampling: Some(serde_json::json!({"top_p": 0.9, "temperature": 0.7})),
            extra_body: Some(
                serde_json::json!({"chat_template_kwargs": {"enable_thinking": false}}),
            ),
            tools: None,
        };
        let a = body_bytes(&req).unwrap();
        let b = body_bytes(&req).unwrap();
        assert_eq!(a, b, "request bytes must be deterministic");
        let s = String::from_utf8(a).unwrap();
        // sorted top-level keys (BTreeMap serialization)
        let extra = s.find("\"extra_body\"").unwrap();
        let max_tokens = s.find("\"max_tokens\"").unwrap();
        let messages = s.find("\"messages\"").unwrap();
        assert!(extra < max_tokens && max_tokens < messages);
        // sampling overrides land top-level; thinking toggle nested
        assert!(s.contains(r#""temperature":0.7"#));
        assert!(s.contains(r#""enable_thinking":false"#));
    }

    #[test]
    fn non_whitelisted_sampling_key_rejected() {
        let req = ChatRequest {
            model: "m".into(),
            messages: vec![],
            max_tokens: 1,
            continuous_usage: false,
            sampling: Some(serde_json::json!({"best_of": 4})),
            extra_body: None,
            tools: None,
        };
        assert!(matches!(body_bytes(&req), Err(LlmError::BadJson(_))));
    }
}
