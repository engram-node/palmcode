//! palmcode-agent -- M1 session engine: provider config, chat loop (no
//! tools), event sourcing on top of palmcode-store. The UI is a reducer
//! over `Event`s; deltas are coalesced to <= 1 batch per frame interval.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
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

/// GLM-family build mode (deliverable I): explicit tool flow, in GLM's
/// preferred imperative style. Keep byte-stable; variables go to the env block.
const SYSTEM_PROMPT_BUILD_GLM: &str = "You are palmcode, a coding agent running on-device inside an Android app, in BUILD mode.

# Tool flow
1. Orient first: ls or glob the paths the request mentions; grep for the symbols you change.
2. read every file before editing it (the harness REFUSES writes to unread files).
3. Change exactly one concern per call: prefer edit (old_string/new_string) over rewriting whole files. Use apply_patch for multi-file or moved-code changes.
4. After each edit, if the result contains a `syntax:` warning, fix it with a follow-up edit BEFORE doing anything else.
5. Batch independent read/glob/grep/ls calls in one assistant turn; never batch writes.
6. Read tool results are capped; when a result mentions a spill path, grep or read slices from that file instead of repeating the call.

# Limits
- Paths are relative to the workspace root; absolute paths are refused.
- Never read or modify secret files (.env, *.pem, keys) unless the task explicitly requires it.
- Bash/shell/git tools are not available in this build; do not attempt them.
- If a tool result is an error, adapt and continue; never repeat the identical call.

# Style
- Be concise: the answer after a change lists what changed, where, and what remains. Do not restate file contents.";

/// Generic template (non-GLM models): same contract, less prescriptive.
const SYSTEM_PROMPT_BUILD_GENERIC: &str = "You are palmcode, a coding agent running on-device inside an Android app, in BUILD mode.

# Rules
- Orient before acting: inspect files with ls/glob/grep and read them fully before editing.
- The harness refuses writes to files you have not read in this session.
- Prefer edit (smallest region) or apply_patch over full-file rewrites.
- If a tool result contains a `syntax:` warning or an error, correct it with your next tool call.
- Independent read-only calls may be batched in one turn; writes are one concern per call.
- Paths are relative to the workspace root. Never touch secret files (.env, private keys) unless required.
- No bash/shell/git tools exist in this build.

# Style
- Be concise and precise. Report what changed and where. Do not restate file contents.";

fn build_prompt(model: &str) -> &'static str {
    if model.to_ascii_lowercase().contains("glm") {
        SYSTEM_PROMPT_BUILD_GLM
    } else {
        SYSTEM_PROMPT_BUILD_GENERIC
    }
}

/// The volatile tail; anything variable goes HERE, never in the prefix.
fn env_block(workspace: &Path) -> String {
    format!(
        "# Environment\nWorkspace root (all tool paths are relative to it): {}",
        workspace.display()
    )
}

/// Byte-stable prefix + trailing environment block (deliverable I): the
/// volatile workspace path lives at the END so the stable head is the
/// prefix-cache key.
fn system_prompt(tools: Option<&Arc<ToolsCtx>>, workspace: Option<&Path>, model: &str) -> String {
    match (tools, workspace) {
        (Some(_), Some(ws)) => format!("{}\n\n{}", build_prompt(model), env_block(ws)),
        _ => SYSTEM_PROMPT.to_string(),
    }
}

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
    /// M2: a tool call began executing (or was decided against).
    ToolStarted {
        seq: u64,
        session_id: String,
        message_id: i64,
        call_id: String,
        name: String,
        /// primary path argument when the tool has one (card subtitle)
        path: Option<String>,
    },
    ToolFinished {
        seq: u64,
        session_id: String,
        message_id: i64,
        call_id: String,
        ok: bool,
        duration_ms: u64,
        /// first chars of the result/error for the collapsed card
        preview: String,
    },
    /// The model hit an Ask rule; UI shows the sheet and calls
    /// `resolve_approval`. Tool output never triggers this.
    ApprovalRequested {
        seq: u64,
        session_id: String,
        message_id: i64,
        request_id: u64,
        tool: String,
        path: String,
        detail: String,
    },
    ApprovalResolved {
        seq: u64,
        session_id: String,
        request_id: u64,
        approved: bool,
    },
    /// "accept edits for this session" is ON (persistent banner).
    AutoApproveEdits {
        seq: u64,
        session_id: Option<String>,
        on: bool,
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
    #[error("workspace/tools: {0}")]
    Tools(String),
}

// ---------------------------------------------------------------------------

/// How an approval sheet answer applies ("deny" == approved:false + Once).
pub enum ApprovalScope {
    Once,
    Session,
    Project,
}

struct Inner {
    client: Option<Client>,
    cfg: Option<Arc<FrozenCfg>>,
}

/// Per-session tool execution context. `read` is session-scoped by spec;
/// project rules are loaded per workspace once.
pub(crate) struct ToolsCtx {
    workspace_root: PathBuf,
    workspace_id: Option<String>,
    roots: palmcode_tools::RootSet,
    reads: palmcode_tools::ReadCache,
    spill: palmcode_tools::Spiller,
    perms: palmcode_perms::Engine,
}

impl ToolsCtx {
    /// `spill_root` must be app-private storage (the workspace may live in
    /// shared/external storage and goes through permission walkups).
    fn open(
        workspace_root: &Path,
        spill_root: &Path,
        workspace_row: Option<palmcode_store::StoredWorkspace>,
        db_approvals: Vec<(String, String)>,
    ) -> Result<Self, EngineError> {
        let roots = palmcode_tools::RootSet::new(vec![workspace_root.to_path_buf()])
            .map_err(|e| EngineError::Tools(e.to_string()))?;
        let spill = palmcode_tools::Spiller::new(spill_root)
            .map_err(|e| EngineError::Tools(e.to_string()))?;
        let trusted = workspace_row.as_ref().map(|w| w.trusted).unwrap_or(false);
        let perms = palmcode_perms::Engine::load(workspace_root, None, trusted, &db_approvals)
            .map_err(EngineError::Tools)?;
        Ok(Self {
            workspace_root: workspace_root.to_path_buf(),
            workspace_id: workspace_row.map(|w| w.id),
            roots,
            reads: palmcode_tools::ReadCache::new(),
            spill,
            perms,
        })
    }
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
    /// single workspace root for now; M2-E swaps this for the workspaces table
    workspace: Mutex<Option<PathBuf>>,
    /// db's directory (spill + workspaces subdirs hang off it)
    db_dir: PathBuf,
    /// app-private spill base (db's directory), per-session subdirs
    spill_base: PathBuf,
    tools: Mutex<HashMap<String, Arc<ToolsCtx>>>,
    approvals: Arc<Mutex<HashMap<u64, ApprovalTx>>>,
    approval_seq: AtomicU64,
    turn_overrides: Arc<Mutex<TurnOverrides>>,
}

type ApprovalTx = oneshot::Sender<(bool, ApprovalScope)>;

/// Per-request knobs layered over provider defaults (chat-input override UI).
#[derive(Debug, Default, Clone, Copy)]
pub struct TurnOverrides {
    pub max_output_tokens: Option<u32>,
}

/// Size/count of a workspace root (deliverable E list screens).
#[derive(Debug, Default, Clone, Copy)]
pub struct WorkspaceMetrics {
    pub present: bool,
    pub size_bytes: u64,
    pub file_count: u64,
}

/// One preimage'd file's diff for the tool card.
#[derive(Debug, Clone)]
pub struct ToolDiff {
    pub path: String,
    pub diff: String,
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
        let db_dir = db_path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .to_path_buf();
        let spill_base = db_dir.join("spill");
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
            workspace: Mutex::new(None),
            db_dir,
            spill_base,
            tools: Mutex::new(HashMap::new()),
            approvals: Arc::new(Mutex::new(HashMap::new())),
            approval_seq: AtomicU64::new(0),
            turn_overrides: Arc::new(Mutex::new(TurnOverrides::default())),
        };
        // crash recovery: anything left streaming is marked interrupted
        for (session, message_id) in engine.store.mark_all_interrupted()? {
            engine.emitter.emit_now(Event::Interrupted {
                seq: engine.next_seq(),
                session_id: session,
                message_id,
            });
        }
        // a tool left "pending" means the app died mid-execution: never
        // re-run; history replay synthesizes an "unknown outcome" result
        engine.store.mark_pending_tools_interrupted()?;
        // tools survive restarts: re-attach the most recently used workspace
        // (validated again — the SAF copy or the path may be gone)
        if let Some(w) = engine
            .store
            .workspaces_list()?
            .into_iter()
            .find(|w| std::path::Path::new(&w.root_path).is_dir())
        {
            let _ = engine.set_workspace(&w.root_path);
        }
        Ok(engine)
    }

    // ---------- M2: workspace + approvals ----------

    /// Enable tools against a workspace root. Tools stay OFF (pure chat)
    /// until a workspace is set.
    pub fn set_workspace(&self, root: &str) -> Result<(), EngineError> {
        let p = PathBuf::from(root);
        // validate once here so misconfig surfaces immediately
        palmcode_tools::RootSet::new(vec![p.clone()])
            .map_err(|e| EngineError::Tools(e.to_string()))?;
        let trusted = self
            .store
            .workspaces_list()
            .ok()
            .and_then(|ws| ws.into_iter().find(|w| w.root_path == root))
            .map(|w| w.trusted)
            .unwrap_or(false);
        palmcode_perms::Engine::load(&p, None, trusted, &[]).map_err(EngineError::Tools)?;
        self.tools.lock().unwrap().clear(); // roots/perms changed: redo ctxs
        *self.workspace.lock().unwrap() = Some(p);
        let _ = self.store.workspace_touch_by_root(root);
        tracing::info!(workspace = root, "tools enabled (workspace set)");
        Ok(())
    }

    pub fn workspace(&self) -> Option<String> {
        self.workspace
            .lock()
            .unwrap()
            .as_ref()
            .map(|p| p.display().to_string())
    }

    pub fn clear_workspace(&self) {
        *self.workspace.lock().unwrap() = None;
        self.tools.lock().unwrap().clear();
    }

    // ---------- M2 E: workspace registry ----------

    /// App-private dir SAF imports copy into: `<db_dir>/workspaces`.
    pub fn workspace_import_dir(&self) -> PathBuf {
        self.db_dir.join("workspaces")
    }

    /// Register a workspace root (SAF-imported copy or dev path).
    pub fn workspace_register(
        &self,
        name: &str,
        root: &str,
    ) -> Result<palmcode_store::StoredWorkspace, EngineError> {
        let p = PathBuf::from(root);
        palmcode_tools::RootSet::new(vec![p]).map_err(|e| EngineError::Tools(e.to_string()))?;
        Ok(self.store.workspace_upsert(name, root)?)
    }

    pub fn workspaces_list(&self) -> Result<Vec<palmcode_store::StoredWorkspace>, EngineError> {
        Ok(self.store.workspaces_list()?)
    }

    /// M3 trust toggle. Untrusted (default): project config ignored;
    /// trusted: project config may add ask/deny only. Applies to NEW tool
    /// contexts — existing sessions rebuild on next turn start, so flip it
    /// between turns, not mid-turn.
    pub fn workspace_set_trusted(&self, id: &str, trusted: bool) -> Result<(), EngineError> {
        self.store.workspace_set_trusted(id, trusted)?;
        self.tools.lock().unwrap().clear();
        Ok(())
    }

    /// Remove the registry row; files are deleted only under the app-private
    /// import dir (never a user's own folder, even if asked).
    pub fn workspace_remove(&self, id: &str, delete_files: bool) -> Result<(), EngineError> {
        let Some(root) = self.store.workspace_remove(id)? else {
            return Ok(());
        };
        if delete_files {
            let rootp = PathBuf::from(&root);
            let import_dir = self.workspace_import_dir();
            let in_private = rootp
                .canonicalize()
                .ok()
                .zip(import_dir.canonicalize().ok())
                .map(|(r, d)| r.starts_with(d))
                .unwrap_or(false);
            if in_private {
                let _ = std::fs::remove_dir_all(&rootp);
            } else {
                tracing::warn!(root, "refusing to delete files outside the import dir");
            }
        }
        // never leave the live ctx pointing at a removed root
        if self.workspace().as_deref() == Some(root.as_str()) {
            self.clear_workspace();
        }
        Ok(())
    }

    /// Unified diffs for a call's preimages vs. the current file content
    /// (tool card expand, deliverable H). Files without preimages / deleted
    /// workspaces yield an empty diff for their path.
    pub fn tool_diff(&self, call_id: &str) -> Result<Vec<ToolDiff>, EngineError> {
        let mut out = Vec::new();
        for p in self.store.preimages_for(call_id)? {
            let old = match (&p.content, &p.spill_path) {
                (Some(c), _) => c.clone(),
                (None, Some(sp)) => std::fs::read_to_string(sp).unwrap_or_default(),
                (None, None) => String::new(),
            };
            let diff = match &self.workspace.lock().unwrap().clone() {
                Some(ws) => {
                    let cur = std::fs::read_to_string(ws.join(&p.path)).unwrap_or_default();
                    palmcode_tools::diff::unified_diff(&p.path, &old, &cur, 3)
                }
                None => String::new(),
            };
            out.push(ToolDiff { path: p.path, diff });
        }
        Ok(out)
    }

    /// Total size + file count of a root (gitignore-aware walk; counts are
    /// capped so a pathological tree cannot run for minutes).
    pub fn workspace_metrics(&self, root: &str) -> Result<WorkspaceMetrics, EngineError> {
        const MAX_FILES: u64 = 1_000_000;
        let rootp = PathBuf::from(root);
        if !rootp.is_dir() {
            return Ok(WorkspaceMetrics {
                present: false,
                size_bytes: 0,
                file_count: 0,
            });
        }
        let mut size = 0u64;
        let mut count = 0u64;
        for entry in ignore::WalkBuilder::new(&rootp)
            .require_git(false) // SAF imports: .gitignore without .git
            .hidden(false)
            .build()
        {
            let Ok(e) = entry else { continue };
            if e.file_type().map(|t| t.is_file()).unwrap_or(false) {
                count += 1;
                if count > MAX_FILES {
                    break;
                }
                size += e.metadata().map(|m| m.len()).unwrap_or(0);
            }
        }
        Ok(WorkspaceMetrics {
            present: true,
            size_bytes: size,
            file_count: count,
        })
    }

    /// Per-session tools context (lazily built; dies with workspace change).
    fn tools_ctx(&self, session_id: &str) -> Option<Arc<ToolsCtx>> {
        let ws = self.workspace.lock().unwrap().clone()?;
        let mut map = self.tools.lock().unwrap();
        if let Some(ctx) = map.get(session_id) {
            return Some(ctx.clone());
        }
        let spill = self.spill_base.join(session_id);
        let row = self
            .store
            .workspaces_list()
            .ok()?
            .into_iter()
            .find(|w| w.root_path == ws.display().to_string());
        let approvals = row
            .as_ref()
            .and_then(|w| self.store.project_approvals_list(&w.id).ok())
            .unwrap_or_default();
        let ctx = Arc::new(ToolsCtx::open(&ws, &spill, row, approvals).ok()?);
        map.insert(session_id.to_string(), ctx.clone());
        Some(ctx)
    }

    /// UI answer for ApprovalRequested.
    pub fn resolve_approval(&self, request_id: u64, approved: bool, scope: ApprovalScope) {
        let tx = self.approvals.lock().unwrap().remove(&request_id);
        if let Some(tx) = tx {
            let _ = tx.send((approved, scope));
        }
    }

    /// Override the next turns' max output tokens (None = provider default).
    pub fn set_max_output_tokens(&self, v: Option<u32>) {
        self.turn_overrides.lock().unwrap().max_output_tokens = v;
    }

    /// "Accept edits for this session": Ask -> Allow for write tools.
    pub fn set_accept_edits(&self, session_id: &str, on: bool) {
        if on {
            if let Some(ctx) = self.tools.lock().unwrap().get(session_id) {
                ctx.perms.set_accept_edits_for_session();
            }
            if let Some(ctx) = self.tools_ctx(session_id) {
                ctx.perms.set_accept_edits_for_session();
            }
        }
        self.emitter.emit_now(Event::AutoApproveEdits {
            seq: self.next_seq(),
            session_id: Some(session_id.to_string()),
            on,
        });
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
        let tools = self.tools_ctx(session_id);

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
        let mut api_messages = vec![Message::system(system_prompt(
            tools.as_ref(),
            self.workspace.lock().unwrap().as_deref(),
            &cfg.main_model,
        ))];
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
            tools,
        }));
        Ok(assistant_mid)
    }

    /// Replay history; completed tool exchanges become assistant-with-calls
    /// plus tool results. An interrupted tail keeps ONLY its tool calls —
    /// unfinished ones get an "interrupted, outcome unknown" result — so the
    /// model continues instead of blindly re-running; partial text history
    /// is dropped as in M1.
    fn api_history(&self, session_id: &str) -> Result<Vec<Message>, EngineError> {
        let rows = self.store.transcript(session_id)?;
        let mut out = Vec::new();
        for (m, parts) in &rows {
            if m.streaming {
                continue;
            }
            let text = join_kind(parts, "text");
            if m.interrupted && m.role == "assistant" {
                let calls = self.store.tool_calls_for(m.id)?;
                if !calls.is_empty() {
                    let specs: Vec<palmcode_llm::types::ToolCallSpec> = calls
                        .iter()
                        .map(|c| palmcode_llm::types::ToolCallSpec {
                            id: c.id.clone(),
                            name: c.name.clone(),
                            arguments: c.args.clone(),
                        })
                        .collect();
                    out.push(Message::assistant_tool_calls(specs, None));
                    for c in &calls {
                        out.push(Message::tool(
                            c.id.clone(),
                            c.output
                                .clone()
                                .unwrap_or_else(|| "[interrupted, outcome unknown]".into()),
                        ));
                    }
                }
                continue;
            }
            if m.interrupted {
                continue;
            }
            match m.role.as_str() {
                "user" => out.push(Message::user(text)),
                "assistant" => {
                    let calls = self.store.tool_calls_for(m.id)?;
                    if calls.is_empty() {
                        out.push(Message::assistant(text));
                    } else {
                        let specs: Vec<palmcode_llm::types::ToolCallSpec> = calls
                            .iter()
                            .map(|c| palmcode_llm::types::ToolCallSpec {
                                id: c.id.clone(),
                                name: c.name.clone(),
                                arguments: c.args.clone(),
                            })
                            .collect();
                        out.push(Message::assistant_tool_calls(
                            specs,
                            if text.is_empty() { None } else { Some(text) },
                        ));
                        for c in &calls {
                            out.push(Message::tool(
                                c.id.clone(),
                                c.output
                                    .clone()
                                    .unwrap_or_else(|| "[interrupted, outcome unknown]".into()),
                            ));
                        }
                    }
                }
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
        let tools = self.tools_ctx(session_id);
        let mut api_messages = vec![Message::system(system_prompt(
            tools.as_ref(),
            self.workspace.lock().unwrap().as_deref(),
            &cfg.main_model,
        ))];
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
            tools,
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
    tools: Option<Arc<ToolsCtx>>,
}
// Engine outlives every turn task (shutdown joins the runtime), and the
// store is behind internal Mutexes.
unsafe impl Send for TurnTask {}

/// Result of one assistant iteration (one HTTP request).
struct IterationFlow {
    outcome: std::result::Result<palmcode_llm::client::TurnOutcome, LlmError>,
    /// tool calls accumulated this iteration
    tool_calls: Vec<palmcode_llm::types::ToolCallSpec>,
    usage: Option<palmcode_llm::types::Usage>,
    ttft: Option<Duration>,
    cancelled: bool,
}

/// Mutable per-turn accumulators shared across iterations.
struct Merge<'a> {
    text: &'a mut String,
    reasoning: &'a mut String,
    cancel_rx: &'a mut oneshot::Receiver<()>,
}

/// Stream one request, appending into the turn's cumulative full_text /
/// full_reasoning (event snapshots + periodic snapshots use the merged
/// text so the UI sees one continuous assistant message).
async fn stream_once(
    engine: &Engine,
    session_id: &str,
    message_id: i64,
    client: &Client,
    request: &ChatRequest,
    acc: &mut Merge<'_>,
) -> IterationFlow {
    let full_text = &mut *acc.text;
    let full_reasoning = &mut *acc.reasoning;
    let cancel_rx = &mut *acc.cancel_rx;
    let (tx, mut rx) = mpsc::unbounded_channel::<ClientEvent>();
    let chat = client.chat(request, tx);
    tokio::pin!(chat);

    let mut reasoning_closed = false;
    let mut usage: Option<palmcode_llm::types::Usage> = None;
    let mut ttft: Option<Duration> = None;
    let mut last_snapshot_write = Instant::now();
    let mut cancelled = false;
    let mut accumulator = palmcode_llm::accumulator::ToolAccumulator::new();

    // Process one client event (shared by the select loop and the final
    // drain — when the chat future resolves its last deltas may still be
    // queued in the channel and must not be lost).
    macro_rules! handle_event {
        ($ev:expr) => {
            match $ev {
                ClientEvent::FirstToken { ttft: t } => ttft = Some(t),
                ClientEvent::Text(t) => {
                    if !reasoning_closed && !full_reasoning.is_empty() {
                        // reasoning part closes when the answer starts
                        let _ = engine.store.put_part(
                            session_id,
                            message_id,
                            "reasoning",
                            full_reasoning,
                        );
                        reasoning_closed = true;
                    }
                    full_text.push_str(&t);
                    engine.emitter.emit(Event::TextDelta {
                        seq: engine.next_seq(),
                        session_id: session_id.to_string(),
                        message_id,
                        snapshot: full_text.clone(),
                    });
                    if last_snapshot_write.elapsed() >= SNAPSHOT_INTERVAL {
                        let _ = engine
                            .store
                            .put_part(session_id, message_id, "text", full_text);
                        last_snapshot_write = Instant::now();
                    }
                }
                ClientEvent::Reasoning(_, r) => {
                    full_reasoning.push_str(&r);
                    engine.emitter.emit(Event::ReasoningDelta {
                        seq: engine.next_seq(),
                        session_id: session_id.to_string(),
                        message_id,
                        snapshot: full_reasoning.clone(),
                    });
                }
                ClientEvent::Tools(deltas) => {
                    for d in &deltas {
                        accumulator.push(d);
                    }
                }
                ClientEvent::Usage(u) => usage = Some(u),
            }
        };
    }

    let outcome: std::result::Result<palmcode_llm::client::TurnOutcome, LlmError> = loop {
        tokio::select! {
            _ = &mut *cancel_rx => {
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
    IterationFlow {
        outcome,
        tool_calls: accumulator.finish(),
        usage,
        ttft,
        cancelled,
    }
}

const MAX_TOOL_ITERATIONS: u32 = 40;
const CONTEXT_GUARD_FRACTION: f64 = 0.85;

/// bytes/token estimate guiding the hard context guard: EMA-smoothed from
/// usage.prompt_tokens of real turns (deliverable G), 3.5 until calibrated.
fn estimate_tokens(msgs: &[Message], bytes_per_token: f64) -> u64 {
    let mut total = 0usize;
    for m in msgs {
        total += m.content.as_deref().unwrap_or("").len();
        for c in &m.tool_calls {
            total += c.function.arguments.len() + c.function.name.len() + 16;
        }
        if m.tool_call_id.is_some() {
            total += 24;
        }
    }
    (total as f64 / bytes_per_token) as u64
}

/// Serialized bytes of the wire payload that usage.prompt_tokens measures
/// (messages + tool defs; model/params overhead is negligible per request).
fn wire_bytes(request: &ChatRequest) -> u64 {
    let msgs = request
        .messages
        .iter()
        .map(|m| serde_json::to_vec(m).map(|v| v.len()).unwrap_or(0))
        .sum::<usize>();
    let tools = request
        .tools
        .as_ref()
        .map(|t| t.iter().map(|s| s.to_string().len()).sum::<usize>())
        .unwrap_or(0);
    (msgs + tools) as u64
}

const DEFAULT_BPT: f64 = 3.5;
const EMA_ALPHA: f64 = 0.35; // ADR 0011

impl Engine {
    fn ema_bpt(&self, model: &str) -> f64 {
        self.store
            .meta_get(&format!("ema_bpt:{model}"))
            .ok()
            .flatten()
            .and_then(|v| v.parse::<f64>().ok())
            .filter(|v| *v > 0.5 && *v < 50.0)
            .unwrap_or(DEFAULT_BPT)
    }

    fn note_usage_sample(&self, model: &str, bytes: u64, prompt_tokens: u64) {
        if prompt_tokens == 0 || bytes == 0 {
            return;
        }
        let sample = bytes as f64 / prompt_tokens as f64;
        let prev = self.ema_bpt(model);
        let ema = EMA_ALPHA * sample + (1.0 - EMA_ALPHA) * prev;
        let _ = self
            .store
            .meta_set(&format!("ema_bpt:{model}"), &ema.to_string());
    }
}

/// Extract the primary path argument for permission decisions / cards.
fn primary_path(args_json: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(args_json).ok()?;
    v.get("path").and_then(|p| p.as_str()).map(String::from)
}

/// One executed call ready to append as a tool result message.
struct CallResult {
    call: palmcode_llm::types::ToolCallSpec,
    content: String,
}

async fn run_turn(task: TurnTask) {
    let TurnTask {
        session_id,
        message_id,
        client,
        cfg,
        mut api_messages,
        store_ptr,
        mut cancel_rx,
        tools,
    } = task;
    let engine: &Engine = unsafe { &*store_ptr };

    let schemas = tools.as_ref().map(|_| palmcode_tools::schemas());
    let mut full_text = String::new();
    let mut full_reasoning = String::new();
    let mut iterations = 0u32;
    let mut first_ttft: Option<Duration> = None;
    let mut total_prompt: Option<u64> = None;
    let mut total_completion: u64 = 0;
    let mut total_cached: Option<u64> = None;
    let mut saw_usage = false;

    let turn_start = Instant::now();
    let mut cancelled = false;
    // Harness-overhead probe (VERIFY: item 2): end of a tool-call stream to
    // dispatch of the follow-up request, per iteration. println! is dropped
    // on non-debuggable apps (ro.debuggable=0 phones) — logcat via liblog.
    let mut last_stream_end: Option<Instant> = None;

    let outcome: std::result::Result<palmcode_llm::client::TurnOutcome, LlmError> = loop {
        iterations += 1;
        if iterations > MAX_TOOL_ITERATIONS {
            break Err(LlmError::BadJson(format!(
                "tool loop exceeded {MAX_TOOL_ITERATIONS} iterations (runaway guard)"
            )));
        }
        if cancel_rx.is_terminated() {
            cancelled = true;
            break Err(LlmError::Cancelled);
        }
        // hard guard: (roughly) full context -> stop before the model does
        let bpt = engine.ema_bpt(&cfg.main_model);
        if estimate_tokens(&api_messages, bpt) as f64
            >= CONTEXT_GUARD_FRACTION * cfg.context_window as f64
        {
            break Err(LlmError::BadJson(format!(
                "context guard: estimated usage reached {:.0}% of {} tokens - \
                 start a new session",
                CONTEXT_GUARD_FRACTION * 100.0,
                cfg.context_window
            )));
        }

        let max_tokens = engine
            .turn_overrides
            .lock()
            .unwrap()
            .max_output_tokens
            .unwrap_or(cfg.max_output_tokens);
        let request = ChatRequest {
            model: cfg.main_model.clone(),
            messages: api_messages.clone(),
            max_tokens,
            continuous_usage: cfg.continuous_usage,
            sampling: cfg.sampling.clone(),
            extra_body: cfg.extra_body.clone(),
            tools: schemas.clone(),
        };

        if let Some(t) = last_stream_end {
            alog(&format!(
                "OVERHEAD turn_gap_ms={} iter={iterations}",
                t.elapsed().as_millis()
            ));
        }
        let flow = stream_once(
            engine,
            &session_id,
            message_id,
            &client,
            &request,
            &mut Merge {
                text: &mut full_text,
                reasoning: &mut full_reasoning,
                cancel_rx: &mut cancel_rx,
            },
        )
        .await;
        last_stream_end = Some(Instant::now());

        if first_ttft.is_none() {
            first_ttft = flow.ttft;
        }
        if let Some(u) = flow.usage {
            saw_usage = true;
            total_completion += u.completion_tokens as u64;
            total_prompt = Some(u.prompt_tokens as u64); // latest request's prompt size
            if let Some(c) = u.cached_tokens {
                // keep the LAST call's cached count (not a sum): the final
                // request carries the whole history, so its ratio is the
                // honest "prefix cache effectiveness at turn end"
                total_cached = Some(c as u64);
            }
            // G: calibrate the bytes/token EMA from real usage
            engine.note_usage_sample(
                &cfg.main_model,
                wire_bytes(&request),
                u.prompt_tokens as u64,
            );
        }
        cancelled |= flow.cancelled;

        let out = match flow.outcome {
            Ok(o) => o,
            Err(e) => break Err(e),
        };

        let wants_tools = out.finish_reason == "tool_calls" && !flow.tool_calls.is_empty();
        match (wants_tools, &tools) {
            (true, Some(ctx)) => {
                if !full_text.ends_with('\n') && !full_text.is_empty() {
                    full_text.push('\n');
                }
                if !full_reasoning.is_empty() {
                    let _ = engine.store.put_part(
                        &session_id,
                        message_id,
                        "reasoning",
                        &full_reasoning,
                    );
                }
                if !full_text.is_empty() {
                    let _ = engine
                        .store
                        .put_part(&session_id, message_id, "text", &full_text);
                }
                // wire view: assistant with tool calls, then results in order
                api_messages.push(Message::assistant_tool_calls(flow.tool_calls.clone(), None));
                if cancel_rx.is_terminated() {
                    cancelled = true;
                    break Err(LlmError::Cancelled);
                }
                let results =
                    execute_calls(engine, ctx, &session_id, message_id, flow.tool_calls).await;
                for r in results {
                    api_messages.push(Message::tool(r.call.id, r.content));
                }
                // loop for the next model turn
                continue;
            }
            (true, None) => {
                // model emitted tool calls with no workspace/tools on
                let note = "requested tool calls but no workspace is connected; \
                            answering without tools";
                full_text.push_str(&format!("\n\n_{note}_\n"));
                engine.emitter.emit(Event::TextDelta {
                    seq: engine.next_seq(),
                    session_id: session_id.clone(),
                    message_id,
                    snapshot: full_text.clone(),
                });
                break Ok(out);
            }
            (false, _) => break Ok(out),
        }
    };

    // final persistence — after EVERY closed part
    if !full_reasoning.is_empty() {
        let _ = engine
            .store
            .put_part(&session_id, message_id, "reasoning", &full_reasoning);
    }
    match outcome {
        Ok(out) => {
            if !full_text.is_empty() {
                let _ = engine
                    .store
                    .put_part(&session_id, message_id, "text", &full_text);
            }
            let completion = u32::try_from(total_completion).unwrap_or(u32::MAX);
            // suppress silly values: decode windows < 250 ms have no rhythm
            let decode_tps = first_ttft.and_then(|t| {
                let d = turn_start.elapsed().saturating_sub(t).as_secs_f64();
                if d >= 0.25 {
                    Some(completion as f64 / d)
                } else {
                    None
                }
            });
            let _ = engine
                .store
                .finalize_message(&session_id, message_id, completion, false);
            engine.emitter.emit_now(Event::MessageFinished {
                seq: engine.next_seq(),
                session_id: session_id.clone(),
                message_id,
                finish_reason: out.finish_reason,
                ttft_ms: first_ttft.map(|d| d.as_millis() as u64),
                decode_tps,
                prompt_tokens: if saw_usage {
                    total_prompt.map(|v| u32::try_from(v).unwrap_or(u32::MAX))
                } else {
                    None
                },
                completion_tokens: if saw_usage { Some(completion) } else { None },
                cached_tokens: total_cached.map(|v| u32::try_from(v).unwrap_or(u32::MAX)),
            });
        }
        Err(e) => {
            let is_cancel = matches!(e, LlmError::Cancelled) || cancelled;
            if !full_text.is_empty() {
                let _ = engine
                    .store
                    .put_part(&session_id, message_id, "text", &full_text);
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
    // the pinned chat future drops at scope end, closing the connection
    engine.inflight.lock().await.remove(&session_id);
}

// ---------------------------------------------------------------------------
// Tool execution (permissioned, persisted, preimage-captured)
// ---------------------------------------------------------------------------

/// Execute a batch of assistant tool calls. Decisions (perms/approvals,
/// persistence rows, events) stay serial on the orchestrator; an
/// all-read-only batch runs its executions concurrently; any write degrades
/// the batch to serial (read-cache + write ordering).
async fn execute_calls(
    engine: &Engine,
    ctx: &Arc<ToolsCtx>,
    session_id: &str,
    message_id: i64,
    calls: Vec<palmcode_llm::types::ToolCallSpec>,
) -> Vec<CallResult> {
    struct Approved {
        spec: palmcode_llm::types::ToolCallSpec,
        rel: String,
    }
    enum Decided {
        Run(Approved),
        Skip(String), // deny text -> error result, no execution
    }

    let mut decided: Vec<Decided> = Vec::with_capacity(calls.len());
    for c in &calls {
        let rel = primary_path(&c.arguments).unwrap_or_default();
        let d = check_permission(engine, ctx, session_id, message_id, c, &rel).await;
        decided.push(match d {
            Ok(()) => Decided::Run(Approved {
                spec: c.clone(),
                rel,
            }),
            Err(deny) => Decided::Skip(deny),
        });
    }

    let any_write = decided.iter().any(|d| {
        matches!(d, Decided::Run(a) if palmcode_perms::TOOLS_WRITE.contains(&a.spec.name.as_str()))
    });

    // Persistence: every call gets a row (pending) before any execution, so a
    // crash mid-batch marks everything unfinished (never silently re-run).
    for c in &calls {
        let _ =
            engine
                .store
                .tool_call_started(session_id, message_id, &c.id, &c.name, &c.arguments);
    }

    // Execution: (text, is_error, duration_ms, spill_path)
    /// (text, is_error, duration_ms, spill_path)
    type Payload = (String, bool, u64, Option<String>);
    let mut outcomes: Vec<Option<Payload>> = (0..calls.len()).map(|_| None).collect();

    if !any_write {
        // concurrent in-process reads; preimage capture unnecessary
        let mut set = tokio::task::JoinSet::new();
        for (idx, d) in decided.into_iter().enumerate() {
            match d {
                Decided::Skip(deny) => {
                    outcomes[idx] = Some((format!("error: {deny}"), true, 0, None))
                }
                Decided::Run(a) => {
                    let ctx = ctx.clone();
                    set.spawn_blocking(move || (idx, exec_one_owned(a.spec, ctx)));
                }
            }
        }
        while let Some(joined) = set.join_next().await {
            match joined {
                Ok((idx, payload)) => outcomes[idx] = Some(payload),
                Err(e) => {
                    tracing::warn!("read tool task failed: {e}");
                }
            }
        }
    } else {
        // serial; preimages right before each write runs
        for (idx, d) in decided.into_iter().enumerate() {
            match d {
                Decided::Skip(deny) => {
                    outcomes[idx] = Some((format!("error: {deny}"), true, 0, None))
                }
                Decided::Run(a) => {
                    if palmcode_perms::TOOLS_WRITE.contains(&a.spec.name.as_str()) {
                        capture_preimages(engine, ctx, &a.spec, &a.rel);
                    }
                    let ctx2 = ctx.clone();
                    let spec = a.spec.clone();
                    let payload = tokio::task::spawn_blocking(move || exec_one_owned(spec, ctx2))
                        .await
                        .unwrap_or_else(|e| {
                            (format!("error: tool task failed: {e}"), true, 0, None)
                        });
                    outcomes[idx] = Some(payload);
                }
            }
        }
    }

    // Finished rows + events, strictly in call order.
    let mut results = Vec::with_capacity(calls.len());
    for (c, out) in calls.iter().zip(outcomes) {
        let (text, is_error, dur, spill) =
            out.unwrap_or_else(|| ("error: internal tool failure".into(), true, 0, None));
        alog(&format!("TOOL name={} ms={} err={}", c.name, dur, is_error));
        let _ = engine.store.tool_call_finished(
            &c.id,
            !is_error,
            Some(&text),
            spill.as_deref(),
            dur as i64,
        );
        engine.emitter.emit_now(Event::ToolFinished {
            seq: engine.next_seq(),
            session_id: session_id.to_string(),
            message_id,
            call_id: c.id.clone(),
            ok: !is_error,
            duration_ms: dur,
            preview: preview(&text),
        });
        results.push(CallResult {
            call: c.clone(),
            content: text,
        });
    }
    results
}

/// spawn_blocking-owning variant of the tool dispatch (avoids lifetime soup).
fn exec_one_owned(
    spec: palmcode_llm::types::ToolCallSpec,
    ctx: Arc<ToolsCtx>,
) -> (String, bool, u64, Option<String>) {
    let t = Instant::now();
    let tctx = palmcode_tools::Ctx {
        roots: &ctx.roots,
        reads: &ctx.reads,
        spill: &ctx.spill,
    };
    let out = palmcode_tools::run_tool(&spec.name, &spec.arguments, &tctx);
    let dur = t.elapsed().as_millis() as u64;
    match out {
        Ok(capped) => (
            capped.text,
            false,
            dur,
            capped.spilled_to.map(|p| p.display().to_string()),
        ),
        Err(e) => (format!("error: {e}"), true, dur, None),
    }
}

fn preview(text: &str) -> String {
    let mut s: String = text.chars().take(160).collect();
    if text.chars().count() > 160 {
        s.push('…');
    }
    s
}

/// Bench/measurement logging that actually reaches logcat on production
/// phones: ART routes app stdout only for debuggable apps, so println! is
/// /dev/null on our release builds. Direct liblog FFI — no subscriber dep.
#[cfg(target_os = "android")]
fn alog(msg: &str) {
    use std::ffi::{c_char, c_int};
    #[link(name = "log")]
    unsafe extern "C" {
        fn __android_log_write(prio: c_int, tag: *const c_char, text: *const c_char) -> c_int;
    }
    let tag = std::ffi::CString::new("palmcode").unwrap();
    let text = std::ffi::CString::new(msg).unwrap_or_else(|_| {
        std::ffi::CString::new("palmcode: message contained NUL").unwrap()
    });
    // 4 = INFO
    unsafe { __android_log_write(4, tag.as_ptr(), text.as_ptr()) };
}

#[cfg(not(target_os = "android"))]
fn alog(msg: &str) {
    println!("{msg}");
}

/// Ask/deny/resolution for one call. Emits ToolStarted on Allow (execution
/// follows), and the approval events when interactive.
async fn check_permission(
    engine: &Engine,
    ctx: &Arc<ToolsCtx>,
    session_id: &str,
    message_id: i64,
    spec: &palmcode_llm::types::ToolCallSpec,
    rel: &str,
) -> std::result::Result<(), String> {
    let (rel_for_match, abs) = if rel.is_empty() {
        ("<no-path>".to_string(), ctx.workspace_root.clone())
    } else {
        match ctx.roots.resolve(rel, None) {
            Ok(abs) => {
                let r = abs
                    .strip_prefix(&ctx.workspace_root)
                    .map(|p| p.display().to_string().replace('\\', "/"))
                    .unwrap_or_else(|_| rel.to_string());
                (r, abs)
            }
            Err(_) => (rel.to_string(), ctx.workspace_root.join(rel)),
        }
    };
    let decision = ctx.perms.decide(&spec.name, &rel_for_match, &abs);
    match decision {
        palmcode_perms::Decision::Allow => {
            engine.emitter.emit_now(Event::ToolStarted {
                seq: engine.next_seq(),
                session_id: session_id.to_string(),
                message_id,
                call_id: spec.id.clone(),
                name: spec.name.clone(),
                path: if rel.is_empty() {
                    None
                } else {
                    Some(rel.to_string())
                },
            });
            Ok(())
        }
        palmcode_perms::Decision::Deny => {
            engine.emitter.emit_now(Event::ToolStarted {
                seq: engine.next_seq(),
                session_id: session_id.to_string(),
                message_id,
                call_id: spec.id.clone(),
                name: spec.name.clone(),
                path: if rel.is_empty() {
                    None
                } else {
                    Some(rel.to_string())
                },
            });
            Err(format!("denied by permission rules ({rel_for_match})"))
        }
        palmcode_perms::Decision::Ask => {
            // accept-edits flag short-circuits (checked inside decide too for
            // the write tools; here for anything else)
            let request_id = engine.approval_seq.fetch_add(1, Ordering::SeqCst) + 1;
            let (tx, rx) = oneshot::channel::<(bool, ApprovalScope)>();
            engine.approvals.lock().unwrap().insert(request_id, tx);
            engine.emitter.emit_now(Event::ApprovalRequested {
                seq: engine.next_seq(),
                session_id: session_id.to_string(),
                message_id,
                request_id,
                tool: spec.name.clone(),
                path: if rel.is_empty() {
                    rel_for_match.clone()
                } else {
                    rel.to_string()
                },
                detail: spec.arguments.chars().take(300).collect(),
            });
            let (approved, scope) = rx.await.unwrap_or((false, ApprovalScope::Once));
            engine.emitter.emit_now(Event::ApprovalResolved {
                seq: engine.next_seq(),
                session_id: session_id.to_string(),
                request_id,
                approved,
            });
            if !approved {
                return Err("denied by the user".into());
            }
            match scope {
                ApprovalScope::Once => {}
                ApprovalScope::Session => {
                    let _ = ctx.perms.allow_for_session(&spec.name, &rel_for_match);
                }
                ApprovalScope::Project => {
                    let _ = ctx.perms.allow_for_session(&spec.name, &rel_for_match);
                    // M3: project-scoped approvals persist in the app DB keyed
                    // by workspace id — never into the repo (a hostile repo
                    // must not inherit trust via its own files).
                    if let Some(wid) = &ctx.workspace_id {
                        let _ = engine
                            .store
                            .project_approval_add(wid, &spec.name, &rel_for_match);
                    }
                }
            }
            engine.emitter.emit_now(Event::ToolStarted {
                seq: engine.next_seq(),
                session_id: session_id.to_string(),
                message_id,
                call_id: spec.id.clone(),
                name: spec.name.clone(),
                path: if rel.is_empty() {
                    None
                } else {
                    Some(rel.to_string())
                },
            });
            Ok(())
        }
    }
}

/// Capture pre-tool images so the UI can show a diff (deliverable F).
/// Files >512KiB are spilled; apply_patch's whole file list is imaged.
fn capture_preimages(
    engine: &Engine,
    ctx: &Arc<ToolsCtx>,
    spec: &palmcode_llm::types::ToolCallSpec,
    rel: &str,
) {
    const INLINE_CAP: u64 = 512 * 1024;
    let mut targets: Vec<String> = Vec::new();
    if spec.name == "apply_patch" {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&spec.arguments)
            && let Some(p) = v.get("patch").and_then(|p| p.as_str())
        {
            targets = palmcode_tools::patch::mentioned_files(p);
        }
    } else if !rel.is_empty() {
        targets.push(rel.to_string());
    }
    for rel in targets {
        let Ok(abs) = ctx.roots.resolve(&rel, None) else {
            continue;
        };
        let Ok(meta) = std::fs::metadata(&abs) else {
            continue; // new file: nothing to image
        };
        if meta.len() > INLINE_CAP {
            if let Ok(bytes) = std::fs::read(&abs)
                && let Ok(spilled) = ctx.spill.spill_file(&String::from_utf8_lossy(&bytes))
            {
                let _ = engine.store.save_preimage(
                    &spec.id,
                    &rel,
                    None,
                    Some(&spilled.display().to_string()),
                );
                continue;
            }
            let _ = engine.store.save_preimage(&spec.id, &rel, None, None);
        } else if let Ok(content) = std::fs::read_to_string(&abs) {
            let _ = engine
                .store
                .save_preimage(&spec.id, &rel, Some(&content), None);
        }
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prompt_family_switch() {
        assert_eq!(build_prompt("glm-4.6"), SYSTEM_PROMPT_BUILD_GLM);
        assert_eq!(build_prompt("mock-glm"), SYSTEM_PROMPT_BUILD_GLM);
        assert_eq!(build_prompt("claude-x"), SYSTEM_PROMPT_BUILD_GENERIC);
    }

    #[test]
    fn prompt_prefix_byte_stable_env_trails() {
        let glm = format!(
            "{}\n\n{}",
            build_prompt("glm-4.6"),
            env_block(Path::new("/ws/a"))
        );
        let glm2 = format!(
            "{}\n\n{}",
            build_prompt("glm-4.6"),
            env_block(Path::new("/ws/b"))
        );
        // volatile part differs only after the stable head
        let head_len = build_prompt("glm-4.6").len();
        assert_eq!(&glm.as_bytes()[..head_len], &glm2.as_bytes()[..head_len]);
        assert!(glm.starts_with(SYSTEM_PROMPT_BUILD_GLM));
        assert!(glm.ends_with(env_block(Path::new("/ws/a")).as_str()));
        assert!(glm.contains("# Environment\nWorkspace root"));
        // both templates teach the syntax-warning repair loop
        for p in [SYSTEM_PROMPT_BUILD_GLM, SYSTEM_PROMPT_BUILD_GENERIC] {
            assert!(p.contains("BUILD mode"));
            assert!(p.contains("syntax:"));
        }
        // no workspace -> old chunk-only prompt
        assert_eq!(system_prompt(None, None, "glm-4.6"), SYSTEM_PROMPT);
    }
}
