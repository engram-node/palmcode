//! Prompt token accounting (docs/VLLM.md): `/tokenize` first, chars/3.5
//! heuristic fallback, results always tagged measured vs estimated.

use crate::error::LlmError;
use crate::types::Message;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TokenCount {
    pub tokens: u32,
    /// true = heuristic (server /tokenize unreachable)
    pub estimated: bool,
}

/// chars/3.5 heuristic (+2 per message framing), runes not bytes.
pub fn heuristic_tokens(messages: &[Message]) -> u32 {
    let mut total = 0u64;
    for m in messages {
        total += 2;
        if let Some(c) = &m.content {
            total += (c.chars().count() as f64 / 3.5).ceil() as u64;
        }
        for tc in &m.tool_calls {
            total += 8;
            total += (tc.function.name.chars().count() as f64 / 3.5).ceil() as u64;
            total += (tc.function.arguments.chars().count() as f64 / 3.5).ceil() as u64;
        }
    }
    total as u32
}

pub struct Counter<'a> {
    client: &'a crate::client::Client,
}

impl<'a> Counter<'a> {
    pub fn new(client: &'a crate::client::Client) -> Self {
        Self { client }
    }

    /// /tokenize the rendered prompt (messages serialized exactly like the
    /// request builder does — the count is per-request anyway); on any
    /// failure fall back to the heuristic and mark it estimated.
    pub async fn count(&self, model: &str, messages: &[Message]) -> TokenCount {
        let rendered = serde_json::to_string(messages).unwrap_or_else(|_| String::from("[]"));
        match self.client.tokenize(model, &rendered).await {
            Ok(n) => TokenCount {
                tokens: n,
                estimated: false,
            },
            Err(e) => {
                tracing::debug!("tokenize unavailable, heuristic fallback: {e}");
                TokenCount {
                    tokens: heuristic_tokens(messages),
                    estimated: true,
                }
            }
        }
    }
}

/// History pruning (docs/VLLM.md "Prefix stability" rule 3): batched pair
/// drops at a threshold, never per-turn. `keep_recent` complete turns are
/// always retained. Returns messages to drop from the FRONT (after the
/// system message at index 0, which is never pruned).
pub fn prune_batch(messages: &[Message], prompt_tokens: u32, context_window: u32) -> usize {
    const THRESHOLD: f64 = 0.80;
    const BATCH: f64 = 0.20;
    let ctx = context_window as u64;
    if ctx == 0 || (prompt_tokens as u64) < (ctx as f64 * THRESHOLD) as u64 {
        return 0;
    }
    let drop_target = (ctx as f64 * BATCH) as u64;
    let start = usize::from(matches!(
        messages.first().map(|m| m.role),
        Some(crate::types::Role::System)
    ));
    let mut drop_bytes_tokens = 0u64;
    let mut drop_count = 0usize;
    // drop complete user..assistant exchanges from the front, never the
    // newest turn (needs at least 2 messages remaining after the cut)
    while drop_bytes_tokens < drop_target {
        let remaining = messages.len().saturating_sub(start + drop_count);
        if remaining <= 2 {
            break;
        }
        let pair_end = (start + drop_count + 2).min(messages.len());
        let pair: Vec<Message> = messages[start + drop_count..pair_end].to_vec();
        drop_bytes_tokens += heuristic_tokens(&pair) as u64;
        drop_count += 2;
    }
    drop_count
}

#[allow(dead_code)]
fn unused(_: &LlmError) {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::Role;

    fn convo(pairs: usize) -> Vec<Message> {
        let mut v = vec![Message::system("sys")];
        for i in 0..pairs {
            v.push(Message::user(format!("question {i} {}", "x".repeat(200))));
            v.push(Message::assistant(format!(
                "answer {i} {}",
                "y".repeat(200)
            )));
        }
        v
    }

    #[test]
    fn no_prune_under_threshold() {
        let m = convo(5);
        assert_eq!(prune_batch(&m, 700, 1000), 0);
    }

    #[test]
    fn batched_prune_keeps_system_and_recent() {
        let m = convo(20);
        let drop = prune_batch(&m, heuristic_tokens(&m), 900);
        assert!(
            drop >= 2 && drop.is_multiple_of(2),
            "drops in pairs: {drop}"
        );
        // the caller drains 1..1+drop, keeping the system message at index 0
        let mut kept = vec![m[0].clone()];
        kept.extend(m[1 + drop..].iter().cloned());
        assert_eq!(kept[0].role, Role::System);
        assert!(kept.len() >= 3, "recent turns retained");
    }

    #[test]
    fn heuristic_monotonic() {
        assert!(heuristic_tokens(&[Message::user("x".repeat(350))]) >= 100);
    }
}
