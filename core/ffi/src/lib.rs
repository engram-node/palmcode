//! UniFFI bindings for the palmcode core. Coarse boundary (AGENTS.md):
//! a handful of calls + one event stream (batched by the emitter; the UI
//! never sees per-token FFI traffic).

use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

use palmcode_agent::{Engine, Event, EventSink, ProviderCfg};

uniffi::setup_scaffolding!();

// ---------------------------------------------------------------------------
// M0 surface (kept: phone-probe.ps1 gates on the greeting)
// ---------------------------------------------------------------------------

#[uniffi::export]
pub fn palmcode_core_version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

#[uniffi::export]
pub fn palmcode_greet(who: String) -> String {
    format!(
        "palmcode core {} online, hello {who}",
        env!("CARGO_PKG_VERSION")
    )
}

// ---------------------------------------------------------------------------
// M1 surface
// ---------------------------------------------------------------------------

#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum FfiError {
    #[error("{0}")]
    Failed(String),
}

fn map_err<E: std::fmt::Display>(e: E) -> FfiError {
    FfiError::Failed(e.to_string())
}

#[derive(uniffi::Record)]
pub struct ProviderConfigRecord {
    pub base_url: String,
    pub api_key: String,
    pub main_model: String,
    pub fast_model: String,
    pub context_window: u32,
    pub max_output_tokens: u32,
    /// JSON object or null; empty by default = no sampling params sent.
    pub sampling_json: Option<String>,
    /// JSON object for extra_body (e.g. chat_template_kwargs) or null.
    pub extra_body_json: Option<String>,
    pub continuous_usage: bool,
    pub first_token_secs: u64,
    pub idle_secs: u64,
}

#[derive(uniffi::Record)]
pub struct SessionInfo {
    pub id: String,
    pub title: String,
    pub status: String,
    pub model: String,
    pub created_at_ms: i64,
}

#[derive(uniffi::Record)]
pub struct TranscriptItemRecord {
    pub message_id: i64,
    pub role: String,
    pub text: String,
    pub reasoning: String,
    pub interrupted: bool,
    pub streaming: bool,
    pub tokens: u32,
    pub error: Option<String>,
}

#[derive(uniffi::Record)]
pub struct ProbeReportRecord {
    pub cleartext_warn: bool,
    pub models_ok: bool,
    pub models_detail: String,
    pub model_ids: Vec<String>,
    pub max_model_len: Option<u32>,
    pub tokenize_ok: bool,
    pub tokenize_detail: String,
    pub tool_choice_ok: bool,
    pub tool_choice_detail: String,
    pub reasoning_field: Option<String>,
    pub continuous_usage: bool,
    pub cached_tokens: bool,
    pub ttft_ms: Option<u64>,
    pub sample_tps: Option<f64>,
    pub chat_error: Option<String>,
}

#[derive(uniffi::Enum)]
pub enum CoreEvent {
    SessionCreated {
        seq: u64,
        id: String,
        title: String,
        status: String,
    },
    MessageStarted {
        seq: u64,
        session_id: String,
        message_id: i64,
        role: String,
    },
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
    Compacted {
        seq: u64,
        session_id: String,
        dropped_messages: u64,
    },
    ToolStarted {
        seq: u64,
        session_id: String,
        message_id: i64,
        call_id: String,
        name: String,
        path: Option<String>,
    },
    ToolFinished {
        seq: u64,
        session_id: String,
        message_id: i64,
        call_id: String,
        ok: bool,
        duration_ms: u64,
        preview: String,
    },
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
    AutoApproveEdits {
        seq: u64,
        session_id: Option<String>,
        on: bool,
    },
}

// `foreign`: Kotlin implements this trait; the Rust engine calls in.
#[uniffi::export(foreign)]
pub trait CoreEventListener: Send + Sync {
    fn on_events(&self, events: Vec<CoreEvent>);
}

struct SinkAdapter(Arc<dyn CoreEventListener>);
impl EventSink for SinkAdapter {
    fn on_batch(&self, events: Vec<Event>) {
        let mapped = events
            .into_iter()
            .map(|e| match e {
                Event::SessionCreated { seq, session } => CoreEvent::SessionCreated {
                    seq,
                    id: session.id,
                    title: session.title,
                    status: session.status,
                },
                Event::MessageStarted {
                    seq,
                    session_id,
                    message_id,
                    role,
                } => CoreEvent::MessageStarted {
                    seq,
                    session_id,
                    message_id,
                    role,
                },
                Event::TextDelta {
                    seq,
                    session_id,
                    message_id,
                    snapshot,
                } => CoreEvent::TextDelta {
                    seq,
                    session_id,
                    message_id,
                    snapshot,
                },
                Event::ReasoningDelta {
                    seq,
                    session_id,
                    message_id,
                    snapshot,
                } => CoreEvent::ReasoningDelta {
                    seq,
                    session_id,
                    message_id,
                    snapshot,
                },
                Event::UsageTick {
                    seq,
                    session_id,
                    prompt_tokens,
                    cached_tokens,
                    estimated,
                } => CoreEvent::UsageTick {
                    seq,
                    session_id,
                    prompt_tokens,
                    cached_tokens,
                    estimated,
                },
                Event::MessageFinished {
                    seq,
                    session_id,
                    message_id,
                    finish_reason,
                    ttft_ms,
                    decode_tps,
                    prompt_tokens,
                    completion_tokens,
                    cached_tokens,
                } => CoreEvent::MessageFinished {
                    seq,
                    session_id,
                    message_id,
                    finish_reason,
                    ttft_ms,
                    decode_tps,
                    prompt_tokens,
                    completion_tokens,
                    cached_tokens,
                },
                Event::Interrupted {
                    seq,
                    session_id,
                    message_id,
                } => CoreEvent::Interrupted {
                    seq,
                    session_id,
                    message_id,
                },
                Event::TurnError {
                    seq,
                    session_id,
                    message_id,
                    code,
                    detail,
                } => CoreEvent::TurnError {
                    seq,
                    session_id,
                    message_id,
                    code,
                    detail,
                },
                Event::Compacted {
                    seq,
                    session_id,
                    dropped_messages,
                } => CoreEvent::Compacted {
                    seq,
                    session_id,
                    dropped_messages: dropped_messages as u64,
                },
                Event::ToolStarted {
                    seq,
                    session_id,
                    message_id,
                    call_id,
                    name,
                    path,
                } => CoreEvent::ToolStarted {
                    seq,
                    session_id,
                    message_id,
                    call_id,
                    name,
                    path,
                },
                Event::ToolFinished {
                    seq,
                    session_id,
                    message_id,
                    call_id,
                    ok,
                    duration_ms,
                    preview,
                } => CoreEvent::ToolFinished {
                    seq,
                    session_id,
                    message_id,
                    call_id,
                    ok,
                    duration_ms,
                    preview,
                },
                Event::ApprovalRequested {
                    seq,
                    session_id,
                    message_id,
                    request_id,
                    tool,
                    path,
                    detail,
                } => CoreEvent::ApprovalRequested {
                    seq,
                    session_id,
                    message_id,
                    request_id,
                    tool,
                    path,
                    detail,
                },
                Event::ApprovalResolved {
                    seq,
                    session_id,
                    request_id,
                    approved,
                } => CoreEvent::ApprovalResolved {
                    seq,
                    session_id,
                    request_id,
                    approved,
                },
                Event::AutoApproveEdits {
                    seq,
                    session_id,
                    on,
                } => CoreEvent::AutoApproveEdits {
                    seq,
                    session_id,
                    on,
                },
            })
            .collect();
        self.0.on_events(mapped);
    }
}

static ENGINE: OnceLock<Engine> = OnceLock::new();

fn engine() -> Result<&'static Engine, FfiError> {
    ENGINE
        .get()
        .ok_or_else(|| FfiError::Failed("core not started".into()))
}

#[uniffi::export]
pub fn core_start(db_path: String, listener: Arc<dyn CoreEventListener>) -> Result<(), FfiError> {
    let sink = Arc::new(SinkAdapter(listener)) as Arc<dyn EventSink>;
    let engine = Engine::start(&PathBuf::from(db_path), sink).map_err(map_err)?;
    let _ = ENGINE.set(engine);
    Ok(())
}

/// Replace the provider configuration; the previous key is dropped and
/// zeroized with the old client. The key never touches disk/logs.
#[uniffi::export]
pub fn provider_configure(cfg: ProviderConfigRecord) -> Result<(), FfiError> {
    engine()?
        .configure(ProviderCfg {
            base_url: cfg.base_url,
            api_key: cfg.api_key,
            main_model: cfg.main_model,
            fast_model: cfg.fast_model,
            context_window: cfg.context_window,
            max_output_tokens: cfg.max_output_tokens,
            sampling_json: cfg.sampling_json,
            extra_body_json: cfg.extra_body_json,
            continuous_usage: cfg.continuous_usage,
            first_token_secs: cfg.first_token_secs,
            idle_secs: cfg.idle_secs,
        })
        .map_err(map_err)
}

/// Blocking network call: Kotlin must invoke it off the main thread
/// (Dispatchers.IO). Returns the full capability panel.
#[uniffi::export]
pub fn provider_probe() -> Result<ProbeReportRecord, FfiError> {
    let r = engine()?.probe().map_err(map_err)?;
    Ok(ProbeReportRecord {
        cleartext_warn: r.cleartext == palmcode_llm::client::Cleartext::WarnLan,
        models_ok: r.models.ok,
        models_detail: r.models.detail,
        model_ids: r.model_ids,
        max_model_len: r.max_model_len,
        tokenize_ok: r.tokenize.ok,
        tokenize_detail: r.tokenize.detail,
        tool_choice_ok: r.tool_choice.ok,
        tool_choice_detail: r.tool_choice.detail,
        reasoning_field: r.reasoning_field.map(|s| s.to_string()),
        continuous_usage: r.continuous_usage,
        cached_tokens: r.cached_tokens_exposed,
        ttft_ms: r.ttft.map(|d| d.as_millis() as u64),
        sample_tps: r.sample_tps,
        chat_error: r.chat,
    })
}

#[uniffi::export]
pub fn sessions_create(title: String) -> Result<SessionInfo, FfiError> {
    let s = engine()?.create_session(&title).map_err(map_err)?;
    Ok(SessionInfo {
        id: s.id,
        title: s.title,
        status: s.status,
        model: s.model,
        created_at_ms: s.created_at,
    })
}

#[uniffi::export]
pub fn sessions_list() -> Result<Vec<SessionInfo>, FfiError> {
    Ok(engine()?
        .list_sessions()
        .map_err(map_err)?
        .into_iter()
        .map(|s| SessionInfo {
            id: s.id,
            title: s.title,
            status: s.status,
            model: s.model,
            created_at_ms: s.created_at,
        })
        .collect())
}

#[uniffi::export]
pub fn session_transcript(session_id: String) -> Result<Vec<TranscriptItemRecord>, FfiError> {
    Ok(engine()?
        .transcript(&session_id)
        .map_err(map_err)?
        .into_iter()
        .map(|t| TranscriptItemRecord {
            message_id: t.message_id,
            role: t.role,
            text: t.text,
            reasoning: t.reasoning,
            interrupted: t.interrupted,
            streaming: t.streaming,
            tokens: t.tokens,
            error: t.error,
        })
        .collect())
}

/// Queue a user message; returns the assistant message id. Progress comes
/// through the listener.
#[uniffi::export]
pub fn message_send(session_id: String, text: String) -> Result<i64, FfiError> {
    engine()?.send(&session_id, &text).map_err(map_err)
}

#[uniffi::export]
pub fn turn_stop(session_id: String) {
    let _ = engine().map(|e| e.stop(&session_id));
}

#[uniffi::export]
pub fn turn_resume(session_id: String) -> Result<(), FfiError> {
    engine()?.resume(&session_id).map_err(map_err)
}

// ---------------------------------------------------------------------------
// M2: workspace + tool approvals
// ---------------------------------------------------------------------------

/// Enable file tools against a workspace root (= turn on agent mode).
#[uniffi::export]
pub fn workspace_set(root: String) -> Result<(), FfiError> {
    engine()?.set_workspace(&root).map_err(map_err)
}

#[uniffi::export]
pub fn workspace_current() -> Option<String> {
    engine().ok().and_then(|e| e.workspace())
}

#[uniffi::export]
pub fn workspace_clear() {
    if let Ok(e) = engine() {
        e.clear_workspace();
    }
}

#[derive(uniffi::Record)]
pub struct WorkspaceRecord {
    pub id: String,
    pub name: String,
    pub root_path: String,
    pub created_at: i64,
    pub last_used_at: i64,
}

#[derive(uniffi::Record)]
pub struct WorkspaceMetricsRecord {
    pub present: bool,
    pub size_bytes: u64,
    pub file_count: u64,
}

fn to_record(w: palmcode_store::StoredWorkspace) -> WorkspaceRecord {
    WorkspaceRecord {
        id: w.id,
        name: w.name,
        root_path: w.root_path,
        created_at: w.created_at,
        last_used_at: w.last_used_at,
    }
}

/// App-private dir the SAF import copies trees into.
#[uniffi::export]
pub fn workspace_import_dir() -> Result<String, FfiError> {
    Ok(engine()?.workspace_import_dir().display().to_string())
}

#[uniffi::export]
pub fn workspace_register(name: String, root_path: String) -> Result<WorkspaceRecord, FfiError> {
    engine()?
        .workspace_register(&name, &root_path)
        .map(to_record)
        .map_err(map_err)
}

#[uniffi::export]
pub fn workspaces_list() -> Result<Vec<WorkspaceRecord>, FfiError> {
    Ok(engine()?
        .workspaces_list()
        .map_err(map_err)?
        .into_iter()
        .map(to_record)
        .collect())
}

/// delete_files is honored only for roots under the app-private import dir.
#[uniffi::export]
pub fn workspace_remove(id: String, delete_files: bool) -> Result<(), FfiError> {
    engine()?
        .workspace_remove(&id, delete_files)
        .map_err(map_err)
}

#[uniffi::export]
pub fn workspace_metrics(root_path: String) -> Result<WorkspaceMetricsRecord, FfiError> {
    let m = engine()?.workspace_metrics(&root_path).map_err(map_err)?;
    Ok(WorkspaceMetricsRecord {
        present: m.present,
        size_bytes: m.size_bytes,
        file_count: m.file_count,
    })
}

/// Answer a pending approval sheet. scope: "once" | "session" | "project".
#[uniffi::export]
pub fn approval_resolve(request_id: u64, approved: bool, scope: String) {
    let s = match scope.as_str() {
        "session" => palmcode_agent::ApprovalScope::Session,
        "project" => palmcode_agent::ApprovalScope::Project,
        _ => palmcode_agent::ApprovalScope::Once,
    };
    if let Ok(e) = engine() {
        e.resolve_approval(request_id, approved, s);
    }
}

/// "Accept edits for this session" toggle (opt-in, always visible).
#[uniffi::export]
pub fn session_set_accept_edits(session_id: String, on: bool) {
    if let Ok(e) = engine() {
        e.set_accept_edits(&session_id, on);
    }
}

#[uniffi::export]
pub fn turn_set_max_output_tokens(max: Option<u32>) {
    if let Ok(e) = engine() {
        e.set_max_output_tokens(max);
    }
}

#[uniffi::export]
pub fn core_shutdown() {
    if let Ok(e) = engine() {
        e.shutdown();
    }
}

// ---------------------------------------------------------------------------
// Android TLS: rustls-platform-verifier needs the app Context once at
// startup. Called from Kotlin as PalmcodeCore.nativeInitAndroid(context).
// ---------------------------------------------------------------------------

#[cfg(target_os = "android")]
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_palmcode_app_PalmcodeCore_nativeInitAndroid(
    mut env: jni::JNIEnv,
    _class: jni::objects::JClass,
    context: jni::objects::JObject,
) -> jni::sys::jint {
    match rustls_platform_verifier::android::init_hosted(&mut env, context) {
        Ok(()) => 0,
        Err(e) => {
            android_log_error(&format!("rustls-platform-verifier init failed: {e}"));
            -1
        }
    }
}

#[cfg(target_os = "android")]
fn android_log_error(msg: &str) {
    // tracing isn't wired yet at init time; keep it in logcat via println
    println!("palmcode-ffi: {msg}");
}

#[cfg(test)]
mod tests {
    #[test]
    fn greeting_mentions_version() {
        let g = super::palmcode_greet("test".to_string());
        assert!(g.contains(env!("CARGO_PKG_VERSION")));
        assert!(g.ends_with("hello test"));
    }
}
