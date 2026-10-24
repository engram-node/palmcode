//! Streaming vLLM client: connect/first-token/idle timeouts, retry only
//! before the first token, Retry-After honored, cancellation = connection
//! close (dropping the response stream aborts the socket).
//!
//! TLS: platform trust store only — schannel (Windows host), system OpenSSL
//! (Linux CI), rustls-platform-verifier (Android runtime-initialized in
//! core/ffi). NO certificate pinning; http:// is refused except
//! LAN/Tailscale/loopback (warned, not hard-blocked).

use std::fmt;
use std::net::IpAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use futures_util::StreamExt as _;
use tokio::sync::mpsc;

use crate::builder::{self, ChatRequest};
use crate::error::{self, LlmError};
use crate::sse::SseParser;
use crate::types::{self, ModelInfo, ReasoningField, ToolCallDelta, Usage};

pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
pub const DEFAULT_FIRST_TOKEN_TIMEOUT: Duration = Duration::from_secs(120);
pub const DEFAULT_IDLE_STALL_TIMEOUT: Duration = Duration::from_secs(60);
const MAX_RETRIES: usize = 2;
const BACKOFF: [Duration; MAX_RETRIES] = [Duration::from_millis(250), Duration::from_millis(1000)];
const RETRY_AFTER_CAP: Duration = Duration::from_secs(10);

/// API key: moves in memory only; Debug/Display are permanently redacted;
/// the buffer is zeroized on drop. It is only ever placed in the
/// `Authorization` header, which is never logged or stringified.
pub struct SecretKey(String);

impl SecretKey {
    pub fn new(s: String) -> Self {
        Self(s)
    }
    pub(crate) fn with_str<R>(&self, f: impl FnOnce(&str) -> R) -> R {
        f(&self.0)
    }
}
impl Drop for SecretKey {
    fn drop(&mut self) {
        zeroize::Zeroize::zeroize(&mut self.0);
    }
}
impl fmt::Debug for SecretKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("<redacted>")
    }
}

#[derive(Debug, Clone)]
pub struct Timeouts {
    pub first_token: Duration,
    pub idle: Duration,
}

impl Default for Timeouts {
    fn default() -> Self {
        Self {
            first_token: DEFAULT_FIRST_TOKEN_TIMEOUT,
            idle: DEFAULT_IDLE_STALL_TIMEOUT,
        }
    }
}

#[derive(Debug)]
pub struct ClientConfig {
    pub base_url: String,
    pub api_key: SecretKey,
    pub timeouts: Timeouts,
    /// vLLM `--enable-continuous-usage-stats` (probe prefills, user may
    /// override).
    pub continuous_usage: bool,
}

/// Cleartext http:// is only tolerable on link-local networks (LAN,
/// Tailscale 100.64/10, loopback). Anything else is refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cleartext {
    Tls,
    WarnLan,
    DeniedHost,
}

pub fn cleartext_policy(base_url: &str) -> Cleartext {
    let Ok(url) = reqwest::Url::parse(base_url) else {
        return Cleartext::Tls; // BadBaseUrl surfaces on first request
    };
    match url.scheme() {
        "https" => Cleartext::Tls,
        "http" => {
            let host = url.host_str().unwrap_or_default().to_ascii_lowercase();
            if host == "localhost" || host.ends_with(".local") || host.ends_with(".lan") {
                return Cleartext::WarnLan;
            }
            let lan = match host.parse::<IpAddr>() {
                Ok(IpAddr::V4(a)) => {
                    let o = a.octets();
                    a.is_loopback()
                        || a.is_private()
                        || a.is_link_local()
                        || (o[0] == 100 && (o[1] & 0xC0) == 64) // 100.64/10 CGNAT (Tailscale)
                }
                Ok(IpAddr::V6(a)) => {
                    a.is_loopback()
                        || (a.segments()[0] & 0xfe00) == 0xfc00
                        || (a.segments()[0] & 0xffc0) == 0xfe80
                }
                Err(_) => false,
            };
            if lan {
                Cleartext::WarnLan
            } else {
                Cleartext::DeniedHost
            }
        }
        _ => Cleartext::DeniedHost,
    }
}

/// Events emitted while streaming; text fragments are transport-granular
/// here and coalesced one level up (agent emitter).
pub enum ClientEvent {
    /// First content/reasoning/tool byte of the assistant turn.
    FirstToken {
        ttft: Duration,
    },
    Text(String),
    Reasoning(ReasoningField, String),
    Tools(Vec<ToolCallDelta>),
    Usage(Usage),
}

#[derive(Debug)]
pub struct TurnOutcome {
    pub finish_reason: String,
    pub usage: Option<Usage>,
    pub ttft: Option<Duration>,
    pub elapsed: Duration,
    /// Non-empty usage objects seen (probe: >1 ⇒ continuous usage stats).
    pub usage_chunks: u32,
    /// Which reasoning field the deltas carried (probe).
    pub reasoning_field: Option<ReasoningField>,
    /// Retries performed before the first token.
    pub retries: u32,
}

#[derive(Clone)]
pub struct Client {
    http: reqwest::Client,
    base: String,
    key: Arc<SecretKey>,
    timeouts: Timeouts,
    continuous_usage: bool,
}

impl fmt::Debug for Client {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Client")
            .field("base", &self.base)
            .field("key", &"<redacted>")
            .finish()
    }
}

fn http_client() -> Result<reqwest::Client, LlmError> {
    let builder = reqwest::Client::builder()
        .connect_timeout(CONNECT_TIMEOUT)
        .redirect(reqwest::redirect::Policy::none())
        .no_proxy();
    #[cfg(not(target_os = "android"))]
    let builder = builder.use_native_tls();
    #[cfg(target_os = "android")]
    let builder = {
        use rustls_platform_verifier::ConfigVerifierExt;
        let cfg = rustls::ClientConfig::with_platform_verifier()
            .map_err(|e| LlmError::Network(format!("platform verifier: {e}")))?;
        builder.use_preconfigured_tls(cfg)
    };
    builder
        .build()
        .map_err(|e| LlmError::Network(e.to_string()))
}

struct AttemptError {
    err: LlmError,
    retry_after: Option<Duration>,
}

fn retryable(e: &LlmError) -> bool {
    matches!(
        e,
        LlmError::ConnectTimeout(_)
            | LlmError::FirstTokenTimeout(_)
            | LlmError::Network(_)
            | LlmError::Http {
                status: 408 | 409 | 429 | 500..=599,
                ..
            }
    )
}

impl Client {
    pub fn new(cfg: ClientConfig) -> Result<Self, LlmError> {
        let url = reqwest::Url::parse(&cfg.base_url)
            .map_err(|_| LlmError::BadBaseUrl(cfg.base_url.clone()))?;
        if !matches!(url.scheme(), "http" | "https") {
            return Err(LlmError::BadBaseUrl(cfg.base_url.clone()));
        }
        if cleartext_policy(&cfg.base_url) == Cleartext::DeniedHost {
            return Err(LlmError::CleartextDenied(cfg.base_url.clone()));
        }
        let ClientConfig {
            base_url,
            api_key,
            timeouts,
            continuous_usage,
        } = cfg;
        Ok(Self {
            http: http_client()?,
            base: base_url.trim_end_matches('/').to_string(),
            key: Arc::new(api_key),
            timeouts,
            continuous_usage,
        })
    }

    pub fn continuous_usage(&self) -> bool {
        self.continuous_usage
    }

    pub fn cleartext(&self) -> Cleartext {
        cleartext_policy(&self.base)
    }

    pub async fn models(&self) -> Result<Vec<ModelInfo>, LlmError> {
        let resp = self
            .authed(self.http.get(format!("{}/models", self.base)))
            .send()
            .await
            .map_err(|e| self.net_err(e))?;
        if !resp.status().is_success() {
            return Err(error::classify_http(
                resp.status().as_u16(),
                &self.read_body(resp).await,
            ));
        }
        let v: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| LlmError::Decode(e.to_string()))?;
        Ok(types::parse_models(&v))
    }

    /// `POST /tokenize` — Err on ANY failure; the caller decides on the
    /// heuristic fallback (the probe wants the raw status for display).
    pub async fn tokenize(&self, model: &str, prompt: &str) -> Result<u32, LlmError> {
        let resp = self
            .authed(self.http.post(format!("{}/tokenize", self.base)))
            .json(&serde_json::json!({"model": model, "prompt": prompt}))
            .send()
            .await
            .map_err(|e| self.net_err(e))?;
        if !resp.status().is_success() {
            return Err(error::classify_http(
                resp.status().as_u16(),
                &self.read_body(resp).await,
            ));
        }
        let v: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| LlmError::Decode(e.to_string()))?;
        v.get("count")
            .and_then(|c| c.as_u64())
            .map(|c| c as u32)
            .ok_or_else(|| LlmError::Decode("tokenize response missing count".into()))
    }

    fn authed(&self, b: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        self.key
            .with_str(|k| b.header("Authorization", format!("Bearer {k}")))
            .header("Accept", "text/event-stream")
    }

    /// One assistant turn. Cancellation: abort the tokio task running this
    /// future — dropping the in-flight response closes the socket
    /// (covered by `mock_server_observes_client_disconnect`).
    pub async fn chat(
        &self,
        req: &ChatRequest,
        tx: mpsc::UnboundedSender<ClientEvent>,
    ) -> Result<TurnOutcome, LlmError> {
        let started = Instant::now();
        let mut backoff = BACKOFF.iter().copied();
        let mut attempt = 0u32;
        loop {
            attempt += 1;
            match self.chat_attempt(req, &tx).await {
                Ok(mut out) => {
                    out.retries = attempt - 1;
                    out.elapsed = started.elapsed();
                    return Ok(out);
                }
                Err(AttemptError { err, retry_after })
                    if retryable(&err) && attempt <= MAX_RETRIES as u32 =>
                {
                    tracing::debug!(
                        "chat attempt {attempt} failed pre-first-token ({}); retrying",
                        error::code(&err)
                    );
                    let wait = retry_after
                        .or_else(|| backoff.next())
                        .unwrap_or(*BACKOFF.last().unwrap())
                        .min(RETRY_AFTER_CAP);
                    tokio::time::sleep(wait).await;
                }
                Err(AttemptError { err, .. }) => return Err(err),
            }
        }
    }

    async fn chat_attempt(
        &self,
        req: &ChatRequest,
        tx: &mpsc::UnboundedSender<ClientEvent>,
    ) -> Result<TurnOutcome, AttemptError> {
        let no_ra = |err| AttemptError {
            err,
            retry_after: None,
        };
        let body = builder::body_bytes(req).map_err(no_ra)?;
        let t0 = Instant::now();
        // first-token budget covers EVERYTHING up to the first SSE byte:
        // connect, request write, response headers, server prefill time.
        // connect() itself is additionally capped at CONNECT_TIMEOUT.
        let response = tokio::time::timeout(
            self.timeouts.first_token,
            self.authed(self.http.post(format!("{}/chat/completions", self.base)))
                .header("Content-Type", "application/json")
                .body(body)
                .send(),
        )
        .await
        .map_err(|_| {
            no_ra(LlmError::FirstTokenTimeout(
                self.timeouts.first_token.as_secs(),
            ))
        })?
        .map_err(|e| no_ra(self.net_err(e)))?;

        let status = response.status();
        if !status.is_success() {
            let retry_after = response
                .headers()
                .get("retry-after")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.parse::<u64>().ok())
                .map(Duration::from_secs)
                .map(|d| d.min(RETRY_AFTER_CAP));
            let snippet = self.read_body(response).await;
            return Err(AttemptError {
                err: error::classify_http(status.as_u16(), &error::redact(snippet, &self.key)),
                retry_after,
            });
        }

        let mut parser = SseParser::new();
        let mut outcome = TurnOutcome {
            finish_reason: String::new(),
            usage: None,
            ttft: None,
            elapsed: Duration::ZERO,
            usage_chunks: 0,
            reasoning_field: None,
            retries: 0,
        };
        let mut acc = crate::accumulator::ToolAccumulator::new();
        let mut stream = response.bytes_stream();
        let mut saw_finish = false;
        let mut saw_done = false;
        loop {
            // Before the first SSE byte the per-read budget is whatever
            // remains of the first-token timeout; after it, the idle-stall
            // timeout applies between chunks.
            let budget = if outcome.ttft.is_none() {
                self.timeouts
                    .first_token
                    .checked_sub(t0.elapsed())
                    .unwrap_or(Duration::from_millis(1))
            } else {
                self.timeouts.idle
            };
            let item = match tokio::time::timeout(budget, stream.next()).await {
                Ok(Some(Ok(bytes))) => bytes,
                Ok(Some(Err(e))) => return Err(no_ra(self.net_err(e))),
                Ok(None) => break, // server closed the socket
                Err(_) => {
                    return Err(no_ra(if outcome.ttft.is_none() {
                        LlmError::FirstTokenTimeout(self.timeouts.first_token.as_secs())
                    } else {
                        LlmError::IdleStall(self.timeouts.idle.as_secs())
                    }));
                }
            };
            for payload in parser.feed(&item) {
                if payload.trim() == "[DONE]" {
                    saw_done = true;
                    continue;
                }
                if outcome.ttft.is_none() {
                    let ttft = t0.elapsed();
                    outcome.ttft = Some(ttft);
                    let _ = tx.send(ClientEvent::FirstToken { ttft });
                }
                let chunk = types::parse_chunk(&payload).map_err(no_ra)?;
                if let Some(c) = chunk {
                    if c.has_usage_object {
                        outcome.usage_chunks += 1;
                    }
                    if let Some(u) = c.usage {
                        outcome.usage = Some(u.clone());
                        let _ = tx.send(ClientEvent::Usage(u));
                    }
                    if let Some((field, text)) = &c.reasoning {
                        outcome.reasoning_field = Some(*field);
                        let _ = tx.send(ClientEvent::Reasoning(*field, text.clone()));
                    }
                    if let Some(t) = &c.text {
                        let _ = tx.send(ClientEvent::Text(t.clone()));
                    }
                    if !c.tool_deltas.is_empty() {
                        for d in &c.tool_deltas {
                            acc.push(d);
                        }
                        let _ = tx.send(ClientEvent::Tools(c.tool_deltas.clone()));
                    }
                    if let Some(fr) = c.finish_reason {
                        outcome.finish_reason = fr;
                        saw_finish = true;
                    }
                }
            }
        }
        // tolerate a missing [DONE] only when a finish_reason was seen
        if !saw_done && !saw_finish {
            return Err(no_ra(LlmError::PrematureEof));
        }
        let _calls = acc.finish(); // M1: accumulator warm, no tools executed
        if outcome.finish_reason.is_empty() {
            outcome.finish_reason = "stop".into();
        }
        Ok(outcome)
    }

    fn net_err(&self, e: reqwest::Error) -> LlmError {
        let connect_timeout = e.is_connect() && e.is_timeout();
        let msg = error::redact(e.without_url().to_string(), &self.key);
        if connect_timeout {
            LlmError::ConnectTimeout(CONNECT_TIMEOUT.as_secs())
        } else {
            LlmError::Network(msg)
        }
    }

    async fn read_body(&self, resp: reqwest::Response) -> String {
        resp.text()
            .await
            .unwrap_or_default()
            .chars()
            .take(600)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cleartext_matrix() {
        assert_eq!(
            cleartext_policy("https://llm.example.invalid/v1"),
            Cleartext::Tls
        );
        assert_eq!(
            cleartext_policy("http://127.0.0.1:8787/v1"),
            Cleartext::WarnLan
        );
        assert_eq!(
            cleartext_policy("http://192.168.1.10:8000/v1"),
            Cleartext::WarnLan
        );
        assert_eq!(
            cleartext_policy("http://100.101.0.5:8000/v1"),
            Cleartext::WarnLan
        );
        assert_eq!(
            cleartext_policy("http://nas.local:8000/v1"),
            Cleartext::WarnLan
        );
        assert_eq!(
            cleartext_policy("http://8.8.8.8:8000/v1"),
            Cleartext::DeniedHost
        );
        assert_eq!(
            cleartext_policy("http://example.com:8000/v1"),
            Cleartext::DeniedHost
        );
        assert_eq!(cleartext_policy("ftp://x"), Cleartext::DeniedHost);
    }

    #[test]
    fn secret_never_shows() {
        let s = SecretKey::new("sk-secret-123".into());
        assert!(!format!("{s:?}").contains("sk-secret-123"));
        let (a, b) = s.with_str(|k| (k.len(), k.starts_with("sk-")));
        assert_eq!((a, b), (13, true));
        assert!(error::redact("server echoed sk-secret-123 back", &s).contains("<redacted>"));
    }
}
