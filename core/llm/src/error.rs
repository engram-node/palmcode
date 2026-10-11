//! Error taxonomy for the vLLM client. UI maps these to banners;
//! `code()` is the stable wire value (Rust variants may evolve).

use crate::client::SecretKey;

/// Every failure the UI can render. Network errors carry a redacted,
/// key-free message (Authorization header values are never stringified).
#[derive(Debug, thiserror::Error)]
pub enum LlmError {
    #[error("HTTP {status}: {snippet}")]
    Http { status: u16, snippet: String },

    /// Server 400 caused by `tool_choice: "auto"` without
    /// `--enable-auto-tool-choice`. `detail` is the server's own hint.
    #[error("server rejects tool_choice=\"auto\": {detail}")]
    ToolChoiceRejected { detail: String },

    #[error("connect timeout ({0} s)")]
    ConnectTimeout(u64),

    #[error("first token timeout ({0} s)")]
    FirstTokenTimeout(u64),

    #[error("stream stalled: no bytes for {0} s")]
    IdleStall(u64),

    #[error("stream ended without a finish_reason")]
    PrematureEof,

    #[error("network error: {0}")]
    Network(String),

    #[error("cleartext http:// is only allowed for LAN/Tailscale/loopback hosts ({0})")]
    CleartextDenied(String),

    #[error("invalid base url: {0}")]
    BadBaseUrl(String),

    #[error("invalid sampling/extra_body JSON: {0}")]
    BadJson(String),

    #[error("response decode error: {0}")]
    Decode(String),

    #[error("cancelled")]
    Cancelled,
}

/// Stable short codes for the FFI/UI boundary.
pub fn code(e: &LlmError) -> &'static str {
    match e {
        LlmError::Http { status: 401, .. } => "unauthorized",
        LlmError::Http { status: 429, .. } => "rate_limited",
        LlmError::Http { status, .. } if *status >= 500 => "server_error",
        LlmError::Http { .. } => "http_error",
        LlmError::ToolChoiceRejected { .. } => "tool_choice_rejected",
        LlmError::ConnectTimeout(_) => "connect_timeout",
        LlmError::FirstTokenTimeout(_) => "first_token_timeout",
        LlmError::IdleStall(_) => "idle_stall",
        LlmError::PrematureEof => "premature_eof",
        LlmError::Network(_) => "network",
        LlmError::CleartextDenied(_) => "cleartext_denied",
        LlmError::BadBaseUrl(_) => "bad_base_url",
        LlmError::BadJson(_) => "bad_json",
        LlmError::Decode(_) => "decode",
        LlmError::Cancelled => "cancelled",
    }
}

/// Remove any occurrence of the API key from a message before it can reach
/// a log or the UI. Defensive: the key is only ever sent in the
/// `Authorization` header, which we never stringify — but a server could
/// echo it in an error body.
pub(crate) fn redact(msg: impl Into<String>, key: &SecretKey) -> String {
    let msg = msg.into();
    key.with_str(|k| msg.replace(k, "<redacted>"))
}

pub(crate) fn classify_http(status: u16, body: &str) -> LlmError {
    let lower = body.to_ascii_lowercase();
    if status == 400
        && (lower.contains("tool_choice")
            || lower.contains("tool-call-parser")
            || lower.contains("auto tool choice"))
    {
        return LlmError::ToolChoiceRejected {
            detail: body.chars().take(300).collect(),
        };
    }
    LlmError::Http {
        status,
        snippet: body.chars().take(300).collect(),
    }
}
