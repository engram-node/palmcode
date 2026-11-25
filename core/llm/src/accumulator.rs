//! Tool-call delta accumulator (vLLM streams tool calls as fragments:
//! first delta has index+id+name, later deltas carry argument chunks —
//! sometimes interleaved for parallel calls). No tools run in M1, but the
//! accumulator is exercised by the committed fixtures now.

use std::collections::BTreeMap;

use crate::types::{ToolCallDelta, ToolCallSpec};

#[derive(Default)]
struct Building {
    id: Option<String>,
    name: String,
    args: String,
    extra_content: Option<crate::types::ToolMetadata>,
}

#[derive(Default)]
pub struct ToolAccumulator {
    by_index: BTreeMap<u32, Building>,
    /// Fallback slot when deltas arrive without `index` (non-conforming
    /// server); keyed by id when present, else a single slot.
    fallback: Option<(Option<String>, Building)>,
}

impl ToolAccumulator {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, d: &ToolCallDelta) {
        let slot: &mut Building = match d.index {
            Some(i) => self.by_index.entry(i).or_default(),
            None => {
                let want_id = d.id.clone();
                let fb = self
                    .fallback
                    .get_or_insert_with(|| (want_id.clone(), Building::default()));
                // a *different* id on the fallback slot starts a new slot;
                // indexless parallel calls are out of spec, best effort only.
                if let (Some(have), Some(w)) = (&fb.0, &want_id) {
                    if have != w {
                        *fb = (want_id, Building::default());
                    }
                } else if fb.0.is_none() {
                    fb.0 = want_id;
                }
                &mut fb.1
            }
        };
        if let Some(id) = &d.id {
            slot.id = Some(id.clone());
        }
        if let Some(name) = &d.name {
            slot.name.push_str(name);
        }
        if let Some(args) = &d.arguments {
            slot.args.push_str(args);
        }
        if let Some(metadata) = &d.extra_content {
            slot.extra_content = Some(metadata.clone());
        }
    }

    /// Completed calls in index order. Calls with empty name are dropped;
    /// argument text is kept verbatim (validity is a tool-runtime concern).
    pub fn finish(self) -> Vec<ToolCallSpec> {
        let mut out: Vec<ToolCallSpec> = Vec::new();
        let mut push = |b: Building| {
            if b.name.trim().is_empty() {
                return;
            }
            out.push(ToolCallSpec {
                id: b.id.unwrap_or_default(),
                name: b.name,
                arguments: b.args,
                extra_content: b.extra_content,
            });
        };
        for (_, b) in self.by_index {
            push(b);
        }
        if let Some((_, b)) = self.fallback {
            push(b);
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opaque_metadata_survives_early_and_late_parallel_deltas() {
        use crate::types::{Message, ToolMetadata};
        for late in [false, true] {
            let mut acc = ToolAccumulator::new();
            for index in 0..2 {
                let metadata = ToolMetadata(
                    serde_json::json!({"google":{"thought_signature":format!("fixture-signature-{index}")}}),
                );
                acc.push(&ToolCallDelta {
                    index: Some(index),
                    id: Some(format!("call-{index}")),
                    name: Some("read".into()),
                    arguments: Some("{".into()),
                    extra_content: (!late).then_some(metadata.clone()),
                });
                acc.push(&ToolCallDelta {
                    index: Some(index),
                    arguments: Some("}".into()),
                    extra_content: late.then_some(metadata),
                    ..Default::default()
                });
            }
            let wire =
                serde_json::to_value(Message::assistant_tool_calls(acc.finish(), None)).unwrap();
            for index in 0..2 {
                assert_eq!(
                    wire["tool_calls"][index]["extra_content"]["google"]["thought_signature"],
                    format!("fixture-signature-{index}")
                );
                assert_eq!(wire["tool_calls"][index]["function"]["arguments"], "{}");
            }
        }
    }

    #[test]
    fn single_call_indexed() {
        let mut acc = ToolAccumulator::new();
        acc.push(&ToolCallDelta {
            index: Some(0),
            id: Some("call_1".into()),
            name: Some("read".into()),
            arguments: None,
            extra_content: None,
        });
        acc.push(&ToolCallDelta {
            index: Some(0),
            arguments: Some("{\"pa".into()),
            ..Default::default()
        });
        acc.push(&ToolCallDelta {
            index: Some(0),
            arguments: Some("th\":\"/a\"}".into()),
            ..Default::default()
        });
        let calls = acc.finish();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].id, "call_1");
        assert_eq!(calls[0].arguments, "{\"path\":\"/a\"}");
        assert!(serde_json::from_str::<serde_json::Value>(&calls[0].arguments).is_ok());
    }

    #[test]
    fn interleaved_parallel_calls() {
        let mut acc = ToolAccumulator::new();
        // interleaved deltas on index 0 and 1 must not cross-contaminate
        acc.push(&ToolCallDelta {
            index: Some(0),
            id: Some("a".into()),
            name: Some("x".into()),
            arguments: Some("{".into()),
            extra_content: None,
        });
        acc.push(&ToolCallDelta {
            index: Some(1),
            id: Some("b".into()),
            name: Some("y".into()),
            arguments: Some("{\"q\":1".into()),
            extra_content: None,
        });
        acc.push(&ToolCallDelta {
            index: Some(0),
            arguments: Some("\"p\":0}".into()),
            ..Default::default()
        });
        acc.push(&ToolCallDelta {
            index: Some(1),
            arguments: Some("}".into()),
            ..Default::default()
        });
        let calls = acc.finish();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].id, "a");
        assert_eq!(calls[0].arguments, "{\"p\":0}");
        assert_eq!(calls[1].id, "b");
        assert_eq!(calls[1].arguments, "{\"q\":1}");
    }

    #[test]
    fn indexless_id_fallback() {
        let mut acc = ToolAccumulator::new();
        acc.push(&ToolCallDelta {
            index: None,
            id: Some("z".into()),
            name: Some("f".into()),
            arguments: Some("{}".into()),
            extra_content: None,
        });
        let calls = acc.finish();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].id, "z");
    }
}
