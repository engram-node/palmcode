//! Incremental SSE (text/event-stream) parser for the vLLM adapter.
//!
//! Feeding is byte-oriented: chunks may split anywhere (TCP slice, UTF-8
//! boundary, inside `data: ...`), and the parser must still yield exactly
//! the events a single-shot parse would. Event payload = concatenation of
//! all `data:` field lines since the last blank line, joined with "\n".
//! Comment lines starting with ':' are dropped (keepalives).

#[derive(Debug, Default)]
pub struct SseParser {
    /// Undelivered bytes (no line terminator seen yet).
    pending: Vec<u8>,
    /// `data:` field lines accumulated for the event in progress.
    data: Vec<u8>,
    /// True once any `data:` line was seen for the event in progress.
    has_data: bool,
}

impl SseParser {
    pub fn new() -> Self {
        Self::default()
    }

    /// Feed a chunk; return the payloads of all events completed by it.
    pub fn feed(&mut self, chunk: &[u8]) -> Vec<String> {
        let mut out = Vec::new();
        self.pending.extend_from_slice(chunk);
        // Swap the buffer out so `line()` can take &mut self.
        let bytes = std::mem::take(&mut self.pending);
        let mut start = 0;
        for i in 0..bytes.len() {
            if bytes[i] == b'\n' {
                let end = if i > start && bytes[i - 1] == b'\r' {
                    i - 1
                } else {
                    i
                };
                if let Some(ev) = self.line(&bytes[start..end]) {
                    out.push(ev);
                }
                start = i + 1;
            }
        }
        self.pending.extend_from_slice(&bytes[start..]);
        out
    }

    /// Flush on connection close; a trailing unterminated line still counts
    /// as a line (servers do send `data: [DONE]` without final newline).
    pub fn finish(mut self) -> Vec<String> {
        let mut out = Vec::new();
        let bytes = std::mem::take(&mut self.pending);
        if !bytes.is_empty() {
            let end = bytes.len() - usize::from(bytes.ends_with(b"\r"));
            if let Some(ev) = self.line(&bytes[..end]) {
                out.push(ev);
            }
        }
        if let Some(ev) = self.dispatch() {
            out.push(ev);
        }
        out
    }

    fn line(&mut self, raw: &[u8]) -> Option<String> {
        // Lone '\r' is not a terminator here; the blank-line dispatch is the
        // only separator we honor (matches every fixture producer in-tree).
        if raw.is_empty() {
            return self.dispatch();
        }
        if raw[0] == b':' {
            return None; // comment / keepalive
        }
        // SSE field grammar: "data" | "data[:one-space]payload".
        let payload: &[u8] = if raw == b"data" {
            b""
        } else if raw.starts_with(b"data:") {
            let after = &raw[5..];
            match after.first() {
                Some(b' ') => &after[1..],
                _ => after,
            }
        } else {
            return None; // event:, id:, retry: — unused by this API
        };
        if self.has_data {
            self.data.push(b'\n');
        }
        self.data.extend_from_slice(payload);
        self.has_data = true;
        None
    }

    fn dispatch(&mut self) -> Option<String> {
        if !self.has_data {
            return None;
        }
        self.has_data = false;
        let bytes = std::mem::take(&mut self.data);
        // vLLM emits JSON; invalid UTF-8 is a server bug — lossy keeps the
        // pipeline alive and the JSON parse downstream will fail loudly.
        Some(String::from_utf8_lossy(&bytes).into_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Parse `bytes` in one shot (canonical reference parse).
    fn canonical(bytes: &[u8]) -> Vec<String> {
        let mut p = SseParser::new();
        let mut evs = p.feed(bytes);
        evs.extend(p.finish());
        evs
    }

    /// Deterministic LCG — no test-only dependencies in the tree.
    struct Lcg(u64);
    impl Lcg {
        fn next(&mut self, n: usize) -> usize {
            self.0 = self
                .0
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            ((self.0 >> 33) as usize) % n.max(1)
        }
    }

    /// Re-chunk `bytes` at random boundaries; parsed events must be
    /// identical to the canonical parse (the item-8 property test).
    fn assert_rechunk_invariant(bytes: &[u8], trials: usize) {
        let want = canonical(bytes);
        for trial in 0..trials {
            let mut rng = Lcg(0x9E3779B97F4A7C15u64.wrapping_add(trial as u64));
            let mut p = SseParser::new();
            let mut evs = Vec::new();
            let mut pos = 0;
            while pos < bytes.len() {
                // chunks of 1..=24 bytes (plus the final remainder)
                let n = 1 + rng.next(24).min(bytes.len() - pos - 1);
                evs.extend(p.feed(&bytes[pos..pos + n]));
                pos += n;
            }
            evs.extend(p.finish());
            assert_eq!(evs, want, "trial {trial} diverged");
        }
    }

    const FIXTURES: &[(&str, &[u8])] = &[
        ("simple", include_bytes!("../fixtures/vllm/simple.sse")),
        (
            "reasoning",
            include_bytes!("../fixtures/vllm/reasoning.sse"),
        ),
        (
            "tool_call",
            include_bytes!("../fixtures/vllm/tool_call.sse"),
        ),
        (
            "parallel_tools",
            include_bytes!("../fixtures/vllm/parallel_tools.sse"),
        ),
        (
            "tool_result_followup",
            include_bytes!("../fixtures/vllm/tool_result_followup.sse"),
        ),
        (
            "long_prefill",
            include_bytes!("../fixtures/vllm/long_prefill.sse"),
        ),
    ];

    #[test]
    fn fixtures_parse_and_terminate() {
        for (name, bytes) in FIXTURES {
            let evs = canonical(bytes);
            assert!(!evs.is_empty(), "{name}: no events parsed");
            assert_eq!(
                evs.last().map(String::as_str),
                Some("[DONE]"),
                "{name}: no DONE sentinel"
            );
        }
    }

    #[test]
    fn random_rechunk_is_parse_invariant() {
        for (name, bytes) in FIXTURES {
            assert_rechunk_invariant(bytes, 40);
            // smoke: canonical parse is non-trivially sized
            assert!(
                canonical(bytes).len() >= 3,
                "{name}: suspiciously few events"
            );
        }
    }

    #[test]
    fn multiline_data_and_comments() {
        let mut p = SseParser::new();
        let evs = p.feed(b": keepalive\r\ndata: a\ndata: b\n\ndata: [D");
        assert!(evs.len() == 1 && evs[0] == "a\nb");
        let tail = p.feed(b"ONE]\n\n");
        assert_eq!(tail, vec!["[DONE]".to_string()]);
        assert!(p.finish().is_empty());
    }
}
