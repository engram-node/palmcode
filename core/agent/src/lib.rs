//! palmcode-agent -- M1 session engine: provider config, chat loop (no
//! tools), event sourcing on top of palmcode-store. The UI is a reducer
//! over `Event`s; deltas are coalesced to <= 1 batch per frame interval.

use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use palmcode_llm::builder::ChatRequest;
use palmcode_llm::client::{Client, ClientConfig, ClientEvent, SecretKey, Timeouts};
use palmcode_llm::count::{Counter, prune_batch};
use palmcode_llm::error::{self, LlmError};
use palmcode_llm::probe::{self, ProbeReport};
use palmcode_llm::types::Message;
use palmcode_store::{Store, StoredMessage, StoredPart, StoredSession};
use tokio::sync::{Mutex as AsyncMutex, mpsc, oneshot};

/// Coalesce streaming deltas at most every frame (~60 fps budget).
const FLUSH_INTERVAL: Duration = Duration::from_millis(16);
/// Persist a streaming text snapshot at most this often (crash safety
/// without per-token DB writes).
const SNAPSHOT_INTERVAL: Duration = Duration::from_millis(1500);

const SYSTEM_PROMPT: &str = "You are palmcode, a coding assistant running on-device inside an Android app. \
     Be concise and precise. In this build no file/shell tools are available.";

// ---------------------------------------------------------------------------
// Public event model
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub enum Event {
    SessionCreated {
        seq: u64,
        session: SessionSummary,
    },
    MessageStarted {
        seq: u64,
        session_id: String,
        message_id: i64,
        role: String,
    },
    /// Full text so far — the UI replaces, never appends.
    TextDelta {
        seq: u64,
        session_id: String,
        message_id: i64,
        snapshot: String,
    },
    ReasoningDelta {
        seq: u64,
        session_id: String,
        message_id: i64,
        snapshot: String,
    },
    UsageTick {
        seq: u64,
        session_id: String,
        prompt_tokens: u32,
        cached_tokens: Option<u32>,
        estimated: bool,
    },
    MessageFinished {
        seq: u64,
        session_id: String,
        message_id: i64,
        finish_reason: String,
        ttft_ms: Option<u64>,
        decode_tps: Option<f64>,
        prompt_tokens: Option<u32>,
        completion_tokens: Option<u32>,
        cached_tokens: Option<u32>,
    },
    /// `resume` drops the request's interrupted tail and regenerates.
    Interrupted {
        seq: u64,
        session_id: String,
        message_id: i64,
    },
    TurnError {
        seq: u64,
        session_id: String,
        message_id: Option<i64>,
        code: String,
        detail: String,
    },
    /// Batched history pruning fired (prefix-cache breaker — TTFT jump on
    /// the following turn is expected and explained in the UI).
    Compacted {
        seq: u64,
        session_id: String,
        dropped_messages: usize,
    },
}

pub trait EventSink: Send + Sync {
    fn on_batch(&self, events: Vec<Event>);
}

#[derive(Debug, Clone)]
pub struct SessionSummary {
    pub id: String,
    pub title: String,
    pub status: String,
    pub model: String,
    pub created_at: i64,
}

#[derive(Debug, Clone)]
pub struct TranscriptItem {
    pub message_id: i64,
    pub role: String,
    pub text: String,
    pub reasoning: String,
    pub interrupted: bool,
    pub streaming: bool,
    pub tokens: u32,
    pub error: Option<String>,
}

// ---------------------------------------------------------------------------
// Provider configuration (passed by the Kotlin layer; key lives in memory)
// ---------------------------------------------------------------------------

pub struct ProviderCfg {
    pub base_url: String,
    pub api_key: String, // handed over, wrapped as SecretKey, zeroized after config build
    pub main_model: String,
    pub fast_model: String,
    pub context_window: u32,
    pub max_output_tokens: u32,
    pub sampling_json: Option<String>,
    pub extra_body_json: Option<String>,
    pub continuous_usage: bool,
    pub first_token_secs: u64,
    pub idle_secs: u64,
}

#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    #[error("provider not configured")]
    NoProvider,
    #[error("llm: {0}")]
    Llm(#[from] LlmError),
    #[error("store: {0}")]
    Store(#[from] palmcode_store::StoreError),
    #[error("no resumable turn on this session")]
    NothingToResume,
}

// ---------------------------------------------------------------------------

struct Inner {
    client: Option<Client>,
    cfg: Option<Arc<FrozenCfg>>,
}

struct FrozenCfg {
    main_model: String,
    #[allow(dead_code)]
    fast_model: String,
    context_window: u32,
    max_output_tokens: u32,
    continuous_usage: bool,
    sampling: Option<serde_json::Value>,
    extra_body: Option<serde_json::Value>,
}

pub struct Engine {
    rt: tokio::runtime::Runtime,
    store: Store,
    /// slot 0 = seq allocator
    seq: AtomicU64,
    inner: Mutex<Inner>,
    inflight: AsyncMutex<HashMap<String, oneshot::Sender<()>>>,
    emitter: Emitter,
}

impl Engine {
    pub fn start(db_path: &Path, sink: Arc<dyn EventSink>) -> Result<Self, EngineError> {
        let store = Store::open(db_path)?;
        let next = store.next_event_seq() as u64;
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .thread_name("palmcode-core")
            .build()
            .expect("tokio runtime");
        let emitter = Emitter::start(rt.handle().clone(), sink.clone());
        let engine = Self {
            rt,
            store,
            seq: AtomicU64::new(next),
            inner: Mutex::new(Inner {
                client: None,
                cfg: None,
            }),
            inflight: AsyncMutex::new(HashMap::new()),
            emitter,
        };
        // crash recovery: anything left streaming is marked interrupted
        for (session, message_id) in engine.store.mark_all_interrupted()? {
            engine.emitter.emit_now(Event::Interrupted {
                seq: engine.next_seq(),
                session_id: session,
                message_id,
            });
        }
        Ok(engine)
    }

    fn next_seq(&self) -> u64 {
        self.seq.fetch_add(1, Ordering::SeqCst) + 1
    }

    pub fn configure(&self, cfg: ProviderCfg) -> Result<(), EngineError> {
        let parse =
            |label: &str, s: Option<String>| -> Result<Option<serde_json::Value>, EngineError> {
                match s {
                    None => Ok(None),
                    Some(raw) => {
                        let v: serde_json::Value = serde_json::from_str(&raw).map_err(|e| {
                            EngineError::Llm(LlmError::BadJson(format!("{label}: {e}")))
                        })?;
                        Ok(Some(v))
                    }
                }
            };
        let sampling = parse("sampling overrides", cfg.sampling_json)?;
        let extra_body = parse("extra_body", cfg.extra_body_json)?;
        let client = Client::new(ClientConfig {
            base_url: cfg.base_url.clone(),
            api_key: SecretKey::new(cfg.api_key),
            timeouts: Timeouts {
                first_token: Duration::from_secs(cfg.first_token_secs),
                idle: Duration::from_secs(cfg.idle_secs),
            },
            continuous_usage: cfg.continuous_usage,
        })?;
        let frozen = Arc::new(FrozenCfg {
            main_model: cfg.main_model,
            fast_model: cfg.fast_model,
            context_window: cfg.context_window,
            max_output_tokens: cfg.max_output_tokens,
            continuous_usage: cfg.continuous_usage,
            sampling,
            extra_body,
        });
        let mut inner = self.inner.lock().unwrap();
        inner.client = Some(client);
        inner.cfg = Some(frozen);
        Ok(())
    }

    pub fn probe(&self) -> Result<ProbeReport, EngineError> {
        let (client, model) = {
            let inner = self.inner.lock().unwrap();
            let client = inner.client.clone().ok_or(EngineError::NoProvider)?;
            let model = inner
                .cfg
                .as_ref()
                .map(|c| c.main_model.clone())
                .ok_or(EngineError::NoProvider)?;
            (client, model)
        };
        Ok(self
            .rt
            .block_on(async move { probe::probe(&client, &model).await }))
    }

    pub fn create_session(&self, title: &str) -> Result<SessionSummary, EngineError> {
        let model = self
            .inner
            .lock()
            .unwrap()
            .cfg
            .as_ref()
            .map(|c| c.main_model.clone())
            .unwrap_or_default();
        let title = if title.trim().is_empty() {
            "New session"
        } else {
            title
        };
        let s: StoredSession = self.store.new_session(title, &model)?;
        let summary = summary(&s);
        self.emitter.emit_now(Event::SessionCreated {
            seq: self.next_seq(),
            session: summary.clone(),
        });
        Ok(summary)
    }

    pub fn list_sessions(&self) -> Result<Vec<SessionSummary>, EngineError> {
        Ok(self.store.list_sessions()?.iter().map(summary).collect())
    }

    pub fn transcript(&self, session_id: &str) -> Result<Vec<TranscriptItem>, EngineError> {
        let rows = self.store.transcript(session_id)?;
        Ok(rows.iter().map(|(m, parts)| itemize(m, parts)).collect())
    }

    /// Queue a user message and stream the assistant reply. Returns the
    /// assistant message id immediately; progress arrives via the sink.
    pub fn send(&self, session_id: &str, text: &str) -> Result<i64, EngineError> {
        let client_cfg = {
            let inner = self.inner.lock().unwrap();
            match (&inner.client, &inner.cfg) {
                (Some(c), Some(cfg)) => (c.clone(), cfg.clone()),
                _ => return Err(EngineError::NoProvider),
            }
        };
        let (client, cfg) = client_cfg;

        // user turn: stored + finalized right away (it's complete input)
        let user_mid = self.store.begin_message(session_id, "user", false)?;
        self.emitter.emit_now(Event::MessageStarted {
            seq: self.next_seq(),
            session_id: session_id.into(),
            message_id: user_mid,
            role: "user".into(),
        });
        self.store.put_part(session_id, user_mid, "text", text)?;
        self.emitter.emit_now(Event::TextDelta {
            seq: self.next_seq(),
            session_id: session_id.into(),
            message_id: user_mid,
            snapshot: text.to_string(),
        });
        self.store
            .finalize_message(session_id, user_mid, 0, false)?;

        // api history: system + all finalized non-interrupted + this user
        let mut api_messages = vec![Message::system(SYSTEM_PROMPT)];
        api_messages.extend(self.api_history(session_id)?);
        let count_task: tokio::task::JoinHandle<palmcode_llm::count::TokenCount> = {
            let client = client.clone();
            let model = cfg.main_model.clone();
            let am = api_messages.clone();
            self.rt
                .handle()
                .spawn(async move { Counter::new(&client).count(&model, &am).await })
        };
        let count = self
            .rt
            .block_on(count_task)
            .unwrap_or(palmcode_llm::count::TokenCount {
                tokens: palmcode_llm::count::heuristic_tokens(&api_messages),
                estimated: true,
            });
        let dropped = prune_batch(&api_messages, count.tokens, cfg.context_window);
        if dropped > 0 {
            api_messages.drain(1..1 + dropped);
            self.emitter.emit_now(Event::Compacted {
                seq: self.next_seq(),
                session_id: session_id.into(),
                dropped_messages: dropped,
            });
        }
        self.emitter.emit_now(Event::UsageTick {
            seq: self.next_seq(),
            session_id: session_id.into(),
            prompt_tokens: count.tokens,
            cached_tokens: None,
            estimated: count.estimated,
        });

        let assistant_mid = self.store.begin_message(session_id, "assistant", true)?;
        self.store.set_status(session_id, "streaming")?;
        self.emitter.emit_now(Event::MessageStarted {
            seq: self.next_seq(),
            session_id: session_id.into(),
            message_id: assistant_mid,
            role: "assistant".into(),
        });

        let (cancel_tx, cancel_rx) = oneshot::channel::<()>();
        self.rt
            .block_on(self.inflight.lock())
            .insert(session_id.into(), cancel_tx);
        let session_owned = session_id.to_string();
        self.rt.spawn(run_turn(TurnTask {
            session_id: session_owned,
            message_id: assistant_mid,
            client,
            cfg,
            api_messages,
            store_ptr: self as *const Engine,
            cancel_rx,
        }));
        Ok(assistant_mid)
    }

    /// Skip the interrupted tail when rebuilding API history.
    fn api_history(&self, session_id: &str) -> Result<Vec<Message>, EngineError> {
        let rows = self.store.transcript(session_id)?;
        let mut out = Vec::new();
        for (m, parts) in &rows {
            if m.interrupted || m.streaming {
                continue;
            }
            let text = join_kind(parts, "text");
            match m.role.as_str() {
                "user" => out.push(Message::user(text)),
                "assistant" => out.push(Message::assistant(text)),
                _ => {}
            }
        }
        Ok(out)
    }

    /// Prompt cancellation: in-flight chat future is dropped inside
    /// select!, closing the connection; the partial message is marked
    /// interrupted and stays visible.
    pub fn stop(&self, session_id: &str) -> Result<(), EngineError> {
        let sender = self.rt.block_on(self.inflight.lock()).remove(session_id);
        match sender {
            Some(tx) => {
                let _ = tx.send(());
                Ok(())
            }
            None => Ok(()), // nothing in flight — not an error
        }
    }

    /// Re-run the last user turn whose assistant reply was interrupted or
    /// errored. History skips the interrupted assistant message (the UI
    /// still shows it, flagged).
    pub fn resume(&self, session_id: &str) -> Result<(), EngineError> {
        let rows = self.store.transcript(session_id)?;
        // resume = last meaningful message is a user turn whose assistant
        // reply was interrupted/errored
        let tail_interrupted = matches!(
            rows.iter().rev().find(|(m, _)| m.role == "assistant"),
            Some((m, _)) if m.interrupted || m.streaming
        );
        let has_user = rows.iter().any(|(m, _)| m.role == "user");
        if !tail_interrupted || !has_user {
            return Err(EngineError::NothingToResume);
        }
        // reuse the existing final user message: create a fresh assistant
        // turn directly
        let client_cfg = {
            let inner = self.inner.lock().unwrap();
            match (&inner.client, &inner.cfg) {
                (Some(c), Some(cfg)) => (c.clone(), cfg.clone()),
                _ => return Err(EngineError::NoProvider),
            }
        };
        let (client, cfg) = client_cfg;
        let mut api_messages = vec![Message::system(SYSTEM_PROMPT)];
        api_messages.extend(self.api_history(session_id)?);
        let count = palmcode_llm::count::TokenCount {
            tokens: palmcode_llm::count::heuristic_tokens(&api_messages),
            estimated: true,
        };
        let dropped = prune_batch(&api_messages, count.tokens, cfg.context_window);
        if dropped > 0 {
            api_messages.drain(1..1 + dropped);
            self.emitter.emit_now(Event::Compacted {
                seq: self.next_seq(),
                session_id: session_id.into(),
                dropped_messages: dropped,
            });
        }
        let assistant_mid = self.store.begin_message(session_id, "assistant", true)?;
        self.store.set_status(session_id, "streaming")?;
        self.emitter.emit_now(Event::MessageStarted {
            seq: self.next_seq(),
            session_id: session_id.into(),
            message_id: assistant_mid,
            role: "assistant".into(),
        });
        let (cancel_tx, cancel_rx) = oneshot::channel::<()>();
        self.rt
            .block_on(self.inflight.lock())
            .insert(session_id.into(), cancel_tx);
        let session_owned = session_id.to_string();
        self.rt.spawn(run_turn(TurnTask {
            session_id: session_owned,
            message_id: assistant_mid,
            client,
            cfg,
            api_messages,
            store_ptr: self as *const Engine,
            cancel_rx,
        }));
        Ok(())
    }

    pub fn shutdown(&self) {
        self.emitter.flush();
    }
}

fn summary(s: &StoredSession) -> SessionSummary {
    SessionSummary {
        id: s.id.clone(),
        title: s.title.clone(),
        status: s.status.clone(),
        model: s.model.clone(),
        created_at: s.created_at,
    }
}

fn join_kind(parts: &[StoredPart], kind: &str) -> String {
    parts
        .iter()
        .filter(|p| p.kind == kind)
        .map(|p| p.content.as_str())
        .collect::<Vec<_>>()
        .join("")
}

fn itemize(m: &StoredMessage, parts: &[StoredPart]) -> TranscriptItem {
    TranscriptItem {
        message_id: m.id,
        role: m.role.clone(),
        text: join_kind(parts, "text"),
        reasoning: join_kind(parts, "reasoning"),
        interrupted: m.interrupted,
        streaming: m.streaming,
        tokens: m.tokens,
        error: parts
            .iter()
            .find(|p| p.kind == "error")
            .map(|p| p.content.clone()),
    }
}

// ---------------------------------------------------------------------------
// Turn task
// ---------------------------------------------------------------------------

struct TurnTask {
    session_id: String,
    message_id: i64,
    client: Client,
    cfg: Arc<FrozenCfg>,
    api_messages: Vec<Message>,
    store_ptr: *const Engine,
    cancel_rx: oneshot::Receiver<()>,
}
// Engine outlives every turn task (shutdown joins the runtime), and the
// store is behind internal Mutexes.
unsafe impl Send for TurnTask {}

async fn run_turn(task: TurnTask) {
    let TurnTask {
        session_id,
        message_id,
        client,
        cfg,
        api_messages,
        store_ptr,
        mut cancel_rx,
    } = task;
    let engine: &Engine = unsafe { &*store_ptr };

    let request = ChatRequest {
        model: cfg.main_model.clone(),
        messages: api_messages,
        max_tokens: cfg.max_output_tokens,
        continuous_usage: cfg.continuous_usage,
        sampling: cfg.sampling.clone(),
        extra_body: cfg.extra_body.clone(),
        tools: None,
    };

    let (tx, mut rx) = mpsc::unbounded_channel::<ClientEvent>();
    let chat = client.chat(&request, tx);
    tokio::pin!(chat);

    let mut text = String::new();
    let mut reasoning = String::new();
    let mut reasoning_closed = false;
    let mut usage: Option<palmcode_llm::types::Usage> = None;
    let mut ttft: Option<Duration> = None;
    let turn_start = Instant::now();
    let mut last_snapshot_write = Instant::now();
    let mut cancelled = false;

    // Process one client event (shared by the select loop and the final
    // drain — when the chat future resolves its last deltas may still be
    // queued in the channel and must not be lost).
    macro_rules! handle_event {
        ($ev:expr) => {
            match $ev {
                ClientEvent::FirstToken { ttft: t } => ttft = Some(t),
                ClientEvent::Text(t) => {
                    if !reasoning_closed && !reasoning.is_empty() {
                        // reasoning part closes when the answer starts
                        let _ =
                            engine
                                .store
                                .put_part(&session_id, message_id, "reasoning", &reasoning);
                        reasoning_closed = true;
                    }
                    text.push_str(&t);
                    engine.emitter.emit(Event::TextDelta {
                        seq: engine.next_seq(),
                        session_id: session_id.clone(),
                        message_id,
                        snapshot: text.clone(),
                    });
                    if last_snapshot_write.elapsed() >= SNAPSHOT_INTERVAL {
                        let _ = engine
                            .store
                            .put_part(&session_id, message_id, "text", &text);
                        last_snapshot_write = Instant::now();
                    }
                }
                ClientEvent::Reasoning(_, r) => {
                    reasoning.push_str(&r);
                    engine.emitter.emit(Event::ReasoningDelta {
                        seq: engine.next_seq(),
                        session_id: session_id.clone(),
                        message_id,
                        snapshot: reasoning.clone(),
                    });
                }
                ClientEvent::Tools(_) => {} // accumulator is unit/fixture-tested
                ClientEvent::Usage(u) => usage = Some(u),
            }
        };
    }

    let outcome: Result<palmcode_llm::client::TurnOutcome, LlmError> = loop {
        tokio::select! {
            _ = &mut cancel_rx => {
                cancelled = true;
                while let Ok(ev) = rx.try_recv() {
                    handle_event!(ev);
                }
                break Err(LlmError::Cancelled);
            }
            res = &mut chat => {
                // drain any deltas the client queued before resolving
                while let Ok(ev) = rx.try_recv() {
                    handle_event!(ev);
                }
                break res;
            }
            Some(ev) = rx.recv() => handle_event!(ev),
        }
    };
    drop(rx);

    // final persistence — after EVERY closed part
    if !reasoning.is_empty() && !reasoning_closed {
        let _ = engine
            .store
            .put_part(&session_id, message_id, "reasoning", &reasoning);
    }
    match outcome {
        Ok(out) => {
            if !text.is_empty() {
                let _ = engine
                    .store
                    .put_part(&session_id, message_id, "text", &text);
            }
            let completion = out.usage.as_ref().map(|u| u.completion_tokens).unwrap_or(0);
            let decode_tps = ttft.map(|t| {
                let d = out.elapsed.saturating_sub(t).as_secs_f64();
                if d > 0.0 { completion as f64 / d } else { 0.0 }
            });
            let _ = engine
                .store
                .finalize_message(&session_id, message_id, completion, false);
            engine.emitter.emit_now(Event::MessageFinished {
                seq: engine.next_seq(),
                session_id: session_id.clone(),
                message_id,
                finish_reason: out.finish_reason,
                ttft_ms: ttft.map(|d| d.as_millis() as u64),
                decode_tps,
                prompt_tokens: out.usage.as_ref().map(|u| u.prompt_tokens),
                completion_tokens: out.usage.as_ref().map(|u| u.completion_tokens),
                cached_tokens: out.usage.and_then(|u| u.cached_tokens),
            });
        }
        Err(e) => {
            let is_cancel = matches!(e, LlmError::Cancelled) || cancelled;
            if !text.is_empty() {
                let _ = engine
                    .store
                    .put_part(&session_id, message_id, "text", &text);
            }
            if !is_cancel {
                let _ = engine.store.put_part(
                    &session_id,
                    message_id,
                    "error",
                    &format!("{}: {}", error::code(&e), e),
                );
            }
            let _ = engine
                .store
                .finalize_message(&session_id, message_id, 0, true); // interrupted => resumable
            if is_cancel {
                engine.emitter.emit_now(Event::Interrupted {
                    seq: engine.next_seq(),
                    session_id: session_id.clone(),
                    message_id,
                });
            } else {
                engine.emitter.emit_now(Event::TurnError {
                    seq: engine.next_seq(),
                    session_id: session_id.clone(),
                    message_id: Some(message_id),
                    code: error::code(&e).into(),
                    detail: format!("{e}"),
                });
            }
        }
    }
    let _ = engine.store.set_status(&session_id, "idle");
    let _ = usage.is_some();
    let _ = turn_start;
    // the pinned chat future drops at scope end, closing the connection
    engine.inflight.lock().await.remove(&session_id);
}

// ---------------------------------------------------------------------------
// Coalescing emitter
// ---------------------------------------------------------------------------

struct EmitterInner {
    pending: Vec<Event>,
    /// index into pending of the latest coalescible delta per key
    positions: HashMap<(String, &'static str, i64), usize>,
    generation: u64,
}

#[derive(Clone)]
struct Emitter {
    sink: Arc<dyn EventSink>,
    inner: Arc<Mutex<EmitterInner>>,
    wake: Arc<tokio::sync::Notify>,
}

impl Emitter {
    fn start(rt: tokio::runtime::Handle, sink: Arc<dyn EventSink>) -> Self {
        let em = Self {
            sink,
            inner: Arc::new(Mutex::new(EmitterInner {
                pending: Vec::new(),
                positions: HashMap::new(),
                generation: 0,
            })),
            wake: Arc::new(tokio::sync::Notify::new()),
        };
        {
            let em = em.clone();
            rt.spawn(async move {
                loop {
                    tokio::time::sleep(FLUSH_INTERVAL).await;
                    em.flush();
                }
            });
        }
        em
    }

    /// Coalescible delta: replace the previous pending delta for the same
    /// (session, kind, message) — the UI only ever shows the snapshot.
    fn emit(&self, ev: Event) {
        {
            let mut inner = self.inner.lock().unwrap();
            let key = match &ev {
                Event::TextDelta {
                    session_id,
                    message_id,
                    ..
                } => Some((session_id.clone(), "text", *message_id)),
                Event::ReasoningDelta {
                    session_id,
                    message_id,
                    ..
                } => Some((session_id.clone(), "reasoning", *message_id)),
                _ => None,
            };
            if let Some(key) = key {
                if let Some(&pos) = inner.positions.get(&key) {
                    let _ = std::mem::replace(&mut inner.pending[pos], ev);
                    return;
                }
                let pos = inner.pending.len();
                inner.positions.insert(key, pos);
            }
            inner.pending.push(ev);
        }
        self.wake.notify_one();
    }

    /// Terminal/control events flush immediately.
    fn emit_now(&self, ev: Event) {
        self.emit(ev);
        self.flush();
    }

    fn flush(&self) {
        let pending = {
            let mut inner = self.inner.lock().unwrap();
            if inner.pending.is_empty() {
                return;
            }
            inner.generation += 1;
            inner.positions.clear();
            std::mem::take(&mut inner.pending)
        };
        self.sink.on_batch(pending);
    }
}
