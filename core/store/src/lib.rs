//! SQLite (WAL) event store. `events` is append-only and carries a global
//! monotonic seq; sessions/messages/parts are projections updated in the
//! same transaction (crash atomic). Everything user-visible is rebuilt
//! from the projections; the event log is the audit + sync source.

use std::path::Path;
use std::sync::Mutex;

use rusqlite::{Connection, params};

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("sqlite: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

#[derive(Debug, Clone)]
pub struct StoredSession {
    pub id: String,
    pub title: String,
    pub created_at: i64,
    pub status: String, // idle | streaming | interrupted
    pub last_seq: i64,
    pub model: String,
}

#[derive(Debug, Clone)]
pub struct StoredMessage {
    pub id: i64,
    pub session_id: String,
    pub role: String,
    pub created_at: i64,
    /// 1 while the assistant turn producing this message is in flight.
    pub streaming: bool,
    /// set on interrupted assistant messages
    pub interrupted: bool,
    pub tokens: u32,
}

#[derive(Debug, Clone)]
pub struct StoredPart {
    pub id: i64,
    pub message_id: i64,
    pub kind: String, // text | reasoning | metric | error
    pub content: String,
    pub created_at: i64,
}

#[derive(Debug, Clone)]
pub struct Preimage {
    pub path: String,
    pub content: Option<String>,
    pub spill_path: Option<String>,
}

#[derive(Debug, Clone)]
pub struct StoredToolCall {
    pub id: String,
    pub session_id: String,
    pub message_id: i64,
    pub name: String,
    pub args: String,
    pub status: String,
    pub started_at: Option<i64>,
    pub finished_at: Option<i64>,
    pub duration_ms: i64,
    pub output: Option<String>,
    pub spill_path: Option<String>,
}

#[derive(Debug, Clone)]
pub struct StoredWorkspace {
    pub id: String,
    pub name: String,
    pub root_path: String,
    pub created_at: i64,
    pub last_used_at: i64,
    pub trusted: bool,
}

pub struct Store {
    conn: Mutex<Connection>,
}

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS meta(k TEXT PRIMARY KEY, v TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS events(
    seq INTEGER PRIMARY KEY AUTOINCREMENT,
    ts INTEGER NOT NULL,
    session_id TEXT NOT NULL,
    kind TEXT NOT NULL,
    payload TEXT NOT NULL DEFAULT '{}'
);
CREATE TABLE IF NOT EXISTS sessions(
    id TEXT PRIMARY KEY,
    title TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    status TEXT NOT NULL DEFAULT 'idle',
    last_seq INTEGER NOT NULL DEFAULT 0,
    model TEXT NOT NULL DEFAULT ''
);
CREATE TABLE IF NOT EXISTS messages(
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    session_id TEXT NOT NULL REFERENCES sessions(id),
    role TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    streaming INTEGER NOT NULL DEFAULT 0,
    interrupted INTEGER NOT NULL DEFAULT 0,
    tokens INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE IF NOT EXISTS parts(
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    message_id INTEGER NOT NULL REFERENCES messages(id),
    kind TEXT NOT NULL,
    content TEXT NOT NULL,
    created_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_messages_session ON messages(session_id, id);
CREATE INDEX IF NOT EXISTS idx_parts_message ON parts(message_id, id);
-- M2: tool calls. status: pending | done | error | interrupted.
-- `output` is capped; the full payload (if any) lives at spill_path.
CREATE TABLE IF NOT EXISTS tool_calls(
    id TEXT PRIMARY KEY,
    session_id TEXT NOT NULL,
    message_id INTEGER NOT NULL REFERENCES messages(id),
    name TEXT NOT NULL,
    args TEXT NOT NULL DEFAULT '{}',
    status TEXT NOT NULL DEFAULT 'pending',
    started_at INTEGER,
    finished_at INTEGER,
    duration_ms INTEGER NOT NULL DEFAULT 0,
    output TEXT,
    spill_path TEXT
);
CREATE INDEX IF NOT EXISTS idx_tool_calls_message ON tool_calls(message_id);
CREATE TABLE IF NOT EXISTS file_preimages(
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    call_id TEXT NOT NULL REFERENCES tool_calls(id),
    path TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    -- content inline; >512 KiB preimages are spilled to the spill dir and
    -- content is NULL with spill_path set
    content TEXT,
    spill_path TEXT
);
CREATE INDEX IF NOT EXISTS idx_preimages_call ON file_preimages(call_id);
-- M2 (E): registered workspaces (SAF-imported copies or dev paths).
-- M3: trusted — project `.palmcode/config.json` rules apply only when 1,
-- and even then allow-rules are stripped (see perms::Engine::load).
CREATE TABLE IF NOT EXISTS workspaces(
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    root_path TEXT NOT NULL UNIQUE,
    created_at INTEGER NOT NULL,
    last_used_at INTEGER NOT NULL,
    trusted INTEGER NOT NULL DEFAULT 0
);
-- M3: always-for-project approvals, per workspace (no longer written to
-- .palmcode/config.json — a hostile repo must not inherit trust via files).
CREATE TABLE IF NOT EXISTS project_approvals(
    workspace_id TEXT NOT NULL,
    tool TEXT NOT NULL,
    path_glob TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    PRIMARY KEY(workspace_id, tool, path_glob)
);
";

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Additive, idempotent upgrades for DBs written before a column existed
/// (no user_version framework yet —_schema drift so far is exactly one
/// column). New tables ride the `IF NOT EXISTS` batch above instead.
fn migrate(conn: &Connection) -> Result<(), StoreError> {
    let has_trusted = conn
        .prepare("SELECT 1 FROM pragma_table_info('workspaces') WHERE name='trusted'")?
        .exists([])?;
    if !has_trusted {
        conn.execute(
            "ALTER TABLE workspaces ADD COLUMN trusted INTEGER NOT NULL DEFAULT 0",
            [],
        )?;
    }
    Ok(())
}

impl Store {
    pub fn open(path: &Path) -> Result<Self, StoreError> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let conn = Connection::open(path)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "busy_timeout", 5000u32)?;
        conn.pragma_update(None, "foreign_keys", true)?;
        conn.execute_batch(SCHEMA)?;
        migrate(&conn)?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    pub fn open_in_memory() -> Result<Self, StoreError> {
        let conn = Connection::open_in_memory()?;
        conn.execute_batch(SCHEMA)?;
        migrate(&conn)?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    pub fn meta_get(&self, key: &str) -> Result<Option<String>, StoreError> {
        let conn = self.conn.lock().unwrap();
        let v = conn
            .query_row("SELECT v FROM meta WHERE k=?1", params![key], |r| {
                r.get::<_, String>(0)
            })
            .ok();
        Ok(v)
    }

    pub fn meta_set(&self, key: &str, value: &str) -> Result<(), StoreError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO meta(k, v) VALUES (?1, ?2) ON CONFLICT(k) DO UPDATE SET v=excluded.v",
            params![key, value],
        )?;
        Ok(())
    }

    pub fn next_event_seq(&self) -> i64 {
        let conn = self.conn.lock().unwrap();
        conn.query_row("SELECT COALESCE(MAX(seq),0) FROM events", [], |r| r.get(0))
            .unwrap_or(0)
            + 1
    }

    pub fn new_session(&self, title: &str, model: &str) -> Result<StoredSession, StoreError> {
        let conn = self.conn.lock().unwrap();
        let id = format!(
            "s{:x}{:04x}",
            now_ms(),
            std::process::id() as u32 ^ (now_ms() as u32 & 0xffff)
        );
        conn.execute(
            "INSERT INTO sessions(id, title, created_at, model) VALUES (?1,?2,?3,?4)",
            params![id, title, now_ms(), model],
        )?;
        Ok(StoredSession {
            id,
            title: title.to_string(),
            created_at: now_ms(),
            status: "idle".into(),
            last_seq: 0,
            model: model.to_string(),
        })
    }

    pub fn list_sessions(&self) -> Result<Vec<StoredSession>, StoreError> {
        let conn = self.conn.lock().unwrap();
        let mut st = conn.prepare(
            "SELECT id,title,created_at,status,last_seq,model FROM sessions ORDER BY created_at DESC",
        )?;
        let rows = st
            .query_map([], |r| {
                Ok(StoredSession {
                    id: r.get(0)?,
                    title: r.get(1)?,
                    created_at: r.get(2)?,
                    status: r.get(3)?,
                    last_seq: r.get(4)?,
                    model: r.get(5)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Begin a message. `streaming=true` marks an assistant turn in flight.
    /// The matching event lands in the SAME transaction.
    pub fn begin_message(
        &self,
        session: &str,
        role: &str,
        streaming: bool,
    ) -> Result<i64, StoreError> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        tx.execute(
            "INSERT INTO messages(session_id, role, created_at, streaming) VALUES (?1,?2,?3,?4)",
            params![session, role, now_ms(), streaming as i32],
        )?;
        let id = tx.last_insert_rowid();
        tx.execute(
            "INSERT INTO events(ts, session_id, kind, payload) VALUES (?1,?2,?3,?4)",
            params![
                now_ms(),
                session,
                "message_started",
                format!("{{\"message_id\":{id},\"role\":\"{role}\",\"streaming\":{streaming}}}")
            ],
        )?;
        tx.commit()?;
        Ok(id)
    }

    /// Append a part and its event atomically. `content` is cumulative for
    /// streaming kinds (text/reasoning): the LATEST snapshot, not a delta.
    /// An existing open part of the same kind on this message is UPDATED,
    /// so a crashed stream preserves the newest snapshot without a row per
    /// chunk.
    pub fn put_part(
        &self,
        session: &str,
        message_id: i64,
        kind: &str,
        content: &str,
    ) -> Result<i64, StoreError> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        let existing: Option<i64> = tx
            .query_row(
                "SELECT id FROM parts WHERE message_id=?1 AND kind=?2 ORDER BY id DESC LIMIT 1",
                params![message_id, kind],
                |r| r.get(0),
            )
            .ok();
        let part_id = if let Some(id) = existing {
            tx.execute(
                "UPDATE parts SET content=?1, created_at=?2 WHERE id=?3",
                params![content, now_ms(), id],
            )?;
            id
        } else {
            tx.execute(
                "INSERT INTO parts(message_id, kind, content, created_at) VALUES (?1,?2,?3,?4)",
                params![message_id, kind, content, now_ms()],
            )?;
            tx.last_insert_rowid()
        };
        tx.execute(
            "INSERT INTO events(ts, session_id, kind, payload) VALUES (?1,?2,'part',?3)",
            params![
                now_ms(),
                session,
                serde_json::json!({
                    "part_id": part_id,
                    "message_id": message_id,
                    "kind": kind,
                    "content": content,
                })
                .to_string()
            ],
        )?;
        let seq: i64 = tx.query_row("SELECT last_insert_rowid()", [], |r| r.get(0))?;
        tx.execute(
            "UPDATE sessions SET last_seq=?1 WHERE id=?2",
            params![seq, session],
        )?;
        tx.commit()?;
        Ok(seq)
    }

    pub fn finalize_message(
        &self,
        session: &str,
        message_id: i64,
        tokens: u32,
        interrupted: bool,
    ) -> Result<(), StoreError> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        tx.execute(
            "UPDATE messages SET streaming=0, interrupted=?1, tokens=?2 WHERE id=?3",
            params![interrupted as i32, tokens, message_id],
        )?;
        tx.execute(
            "UPDATE sessions SET status=?1 WHERE id=?2",
            params![if interrupted { "interrupted" } else { "idle" }, session],
        )?;
        tx.execute(
            "INSERT INTO events(ts, session_id, kind, payload) VALUES (?1,?2,'message_finished',?3)",
            params![
                now_ms(),
                session,
                serde_json::json!({"message_id": message_id, "tokens": tokens, "interrupted": interrupted}).to_string()
            ],
        )?;
        tx.commit()?;
        Ok(())
    }

    // --- workspaces (M2 E) --------------------------------------------------

    /// Register (or rename) a workspace root. Id is stable per root.
    pub fn workspace_upsert(
        &self,
        name: &str,
        root_path: &str,
    ) -> Result<StoredWorkspace, StoreError> {
        let conn = self.conn.lock().unwrap();
        let id = format!(
            "w{:x}{:04x}",
            now_ms(),
            std::process::id() as u32 ^ (now_ms() as u32 & 0xffff)
        );
        conn.execute(
            "INSERT INTO workspaces(id, name, root_path, created_at, last_used_at) \
             VALUES (?1,?2,?3,?4,?4) \
             ON CONFLICT(root_path) DO UPDATE SET name=excluded.name, last_used_at=excluded.last_used_at",
            params![id, name, root_path, now_ms()],
        )?;
        conn.query_row(
            "SELECT id, name, root_path, created_at, last_used_at, trusted FROM workspaces WHERE root_path=?1",
            params![root_path],
            |r| {
                Ok(StoredWorkspace {
                    id: r.get(0)?,
                    name: r.get(1)?,
                    root_path: r.get(2)?,
                    created_at: r.get(3)?,
                    last_used_at: r.get(4)?,
                    trusted: r.get::<_, i64>(5)? != 0,
                })
            },
        )
        .map_err(StoreError::from)
    }

    pub fn workspaces_list(&self) -> Result<Vec<StoredWorkspace>, StoreError> {
        let conn = self.conn.lock().unwrap();
        let mut st = conn.prepare(
            "SELECT id, name, root_path, created_at, last_used_at, trusted FROM workspaces ORDER BY last_used_at DESC",
        )?;
        let rows = st
            .query_map([], |r| {
                Ok(StoredWorkspace {
                    id: r.get(0)?,
                    name: r.get(1)?,
                    root_path: r.get(2)?,
                    created_at: r.get(3)?,
                    last_used_at: r.get(4)?,
                    trusted: r.get::<_, i64>(5)? != 0,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn workspace_set_trusted(&self, id: &str, trusted: bool) -> Result<(), StoreError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE workspaces SET trusted=?1 WHERE id=?2",
            params![trusted as i64, id],
        )?;
        Ok(())
    }

    /// Project-scoped "always" approvals (M3): allow rules only — sessions
    /// load them as a layer between session grants and project config.
    pub fn project_approvals_list(
        &self,
        workspace_id: &str,
    ) -> Result<Vec<(String, String)>, StoreError> {
        let conn = self.conn.lock().unwrap();
        let mut st =
            conn.prepare("SELECT tool, path_glob FROM project_approvals WHERE workspace_id=?1")?;
        let rows = st
            .query_map(params![workspace_id], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn project_approval_add(
        &self,
        workspace_id: &str,
        tool: &str,
        path_glob: &str,
    ) -> Result<(), StoreError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT OR IGNORE INTO project_approvals(workspace_id, tool, path_glob, created_at) \
             VALUES (?1,?2,?3,?4)",
            params![workspace_id, tool, path_glob, now_ms()],
        )?;
        Ok(())
    }

    /// Bump last_used for a root (set_workspace). Missing row is not an error.
    pub fn workspace_touch_by_root(&self, root_path: &str) -> Result<(), StoreError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE workspaces SET last_used_at=?1 WHERE root_path=?2",
            params![now_ms(), root_path],
        )?;
        Ok(())
    }

    pub fn workspace_remove(&self, id: &str) -> Result<Option<String>, StoreError> {
        let conn = self.conn.lock().unwrap();
        let root: Option<String> = conn
            .query_row(
                "SELECT root_path FROM workspaces WHERE id=?1",
                params![id],
                |r| r.get(0),
            )
            .ok();
        conn.execute("DELETE FROM workspaces WHERE id=?1", params![id])?;
        Ok(root)
    }

    pub fn set_status(&self, session: &str, status: &str) -> Result<(), StoreError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE sessions SET status=?1 WHERE id=?2",
            params![status, session],
        )?;
        Ok(())
    }

    pub fn transcript(
        &self,
        session: &str,
    ) -> Result<Vec<(StoredMessage, Vec<StoredPart>)>, StoreError> {
        let conn = self.conn.lock().unwrap();
        let mut st = conn.prepare(
            "SELECT id, session_id, role, created_at, streaming, interrupted, tokens
             FROM messages WHERE session_id=?1 ORDER BY id",
        )?;
        let msgs = st
            .query_map(params![session], |r| {
                Ok(StoredMessage {
                    id: r.get(0)?,
                    session_id: r.get(1)?,
                    role: r.get(2)?,
                    created_at: r.get(3)?,
                    streaming: r.get::<_, i32>(4)? != 0,
                    interrupted: r.get::<_, i32>(5)? != 0,
                    tokens: r.get::<_, i64>(6)? as u32,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        let mut pst = conn.prepare(
            "SELECT id, message_id, kind, content, created_at FROM parts WHERE message_id=?1 ORDER BY id",
        )?;
        let mut out = Vec::with_capacity(msgs.len());
        for m in msgs {
            let parts = pst
                .query_map(params![m.id], |r| {
                    Ok(StoredPart {
                        id: r.get(0)?,
                        message_id: r.get(1)?,
                        kind: r.get(2)?,
                        content: r.get(3)?,
                        created_at: r.get(4)?,
                    })
                })?
                .collect::<Result<Vec<_>, _>>()?;
            out.push((m, parts));
        }
        Ok(out)
    }

    /// Sessions left with an in-flight message (app killed mid-stream).
    pub fn interrupted_sessions(&self) -> Result<Vec<String>, StoreError> {
        let conn = self.conn.lock().unwrap();
        let mut st = conn.prepare("SELECT DISTINCT session_id FROM messages WHERE streaming=1")?;
        let rows = st
            .query_map([], |r| r.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    // ---------- M2: tool calls + preimages ----------

    pub fn tool_call_started(
        &self,
        session: &str,
        message_id: i64,
        call_id: &str,
        name: &str,
        args: &str,
    ) -> Result<(), StoreError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO tool_calls(id, session_id, message_id, name, args, status, started_at)
             VALUES (?1,?2,?3,?4,?5,'pending',?6)",
            params![call_id, session, message_id, name, args, now_ms()],
        )?;
        Ok(())
    }

    pub fn tool_call_finished(
        &self,
        call_id: &str,
        ok: bool,
        output: Option<&str>,
        spill_path: Option<&str>,
        duration_ms: i64,
    ) -> Result<(), StoreError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE tool_calls SET status=?2, finished_at=?3, duration_ms=?4, output=?5, spill_path=?6
             WHERE id=?1",
            params![
                call_id,
                if ok { "done" } else { "error" },
                now_ms(),
                duration_ms,
                output,
                spill_path
            ],
        )?;
        Ok(())
    }

    pub fn tool_calls_for(&self, message_id: i64) -> Result<Vec<StoredToolCall>, StoreError> {
        let conn = self.conn.lock().unwrap();
        let mut st = conn.prepare(
            "SELECT id, session_id, message_id, name, args, status, started_at, finished_at,
                    duration_ms, output, spill_path
             FROM tool_calls WHERE message_id=?1 ORDER BY started_at",
        )?;
        let rows = st
            .query_map(params![message_id], |r| {
                Ok(StoredToolCall {
                    id: r.get(0)?,
                    session_id: r.get(1)?,
                    message_id: r.get(2)?,
                    name: r.get(3)?,
                    args: r.get(4)?,
                    status: r.get(5)?,
                    started_at: r.get(6)?,
                    finished_at: r.get(7)?,
                    duration_ms: r.get(8)?,
                    output: r.get(9)?,
                    spill_path: r.get(10)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Every call left "pending" — possible only if the app died mid-tool.
    /// The engine marks them interrupted and synthesizes "interrupted,
    /// outcome unknown" tool results; a started call is NEVER re-run.
    pub fn pending_tool_calls(&self) -> Result<Vec<StoredToolCall>, StoreError> {
        let conn = self.conn.lock().unwrap();
        let mut st = conn.prepare(
            "SELECT id, session_id, message_id, name, args, status, started_at, finished_at,
                    duration_ms, output, spill_path
             FROM tool_calls WHERE status='pending' ORDER BY started_at",
        )?;
        let rows = st
            .query_map([], |r| {
                Ok(StoredToolCall {
                    id: r.get(0)?,
                    session_id: r.get(1)?,
                    message_id: r.get(2)?,
                    name: r.get(3)?,
                    args: r.get(4)?,
                    status: r.get(5)?,
                    started_at: r.get(6)?,
                    finished_at: r.get(7)?,
                    duration_ms: r.get(8)?,
                    output: r.get(9)?,
                    spill_path: r.get(10)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn mark_pending_tools_interrupted(&self) -> Result<(), StoreError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE tool_calls SET status='interrupted', finished_at=?1
             WHERE status='pending'",
            params![now_ms()],
        )?;
        Ok(())
    }

    pub fn save_preimage(
        &self,
        call_id: &str,
        path: &str,
        content: Option<&str>,
        spill_path: Option<&str>,
    ) -> Result<(), StoreError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO file_preimages(call_id, path, created_at, content, spill_path)
             VALUES (?1,?2,?3,?4,?5)",
            params![call_id, path, now_ms(), content, spill_path],
        )?;
        Ok(())
    }

    pub fn preimages_for(&self, call_id: &str) -> Result<Vec<Preimage>, StoreError> {
        let conn = self.conn.lock().unwrap();
        let mut st = conn.prepare(
            "SELECT path, content, spill_path FROM file_preimages WHERE call_id=?1 ORDER BY id",
        )?;
        let rows = st
            .query_map(params![call_id], |r| {
                Ok(Preimage {
                    path: r.get(0)?,
                    content: r.get(1)?,
                    spill_path: r.get(2)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// DB file + WAL size (settings screen).
    pub fn db_size_bytes(&self) -> Result<u64, StoreError> {
        let conn = self.conn.lock().unwrap();
        let pages: i64 = conn.query_row("PRAGMA page_count", [], |r| r.get(0))?;
        let page_size: i64 = conn.query_row("PRAGMA page_size", [], |r| r.get(0))?;
        Ok((pages * page_size) as u64)
    }

    /// Retention: drop preimages older than `retained_after_ms`, then
    /// incremental_vacuum if freelist > 20% (settings/boot maintenance).
    pub fn gc(&self, retained_after_ms: i64) -> Result<(u64, bool), StoreError> {
        let conn = self.conn.lock().unwrap();
        let deleted = conn.execute(
            "DELETE FROM file_preimages WHERE created_at < ?1",
            params![retained_after_ms],
        )? as u64;
        let total: i64 = conn.query_row("PRAGMA page_count", [], |r| r.get(0))?;
        let free: i64 = conn.query_row("PRAGMA freelist_count", [], |r| r.get(0))?;
        let vacuumed = if total > 0 && free * 5 > total {
            conn.execute_batch("PRAGMA incremental_vacuum")?;
            true
        } else {
            false
        };
        Ok((deleted, vacuumed))
    }

    /// Mark every still-streaming message interrupted (crash recovery at
    /// engine start). Returns affected message ids.
    pub fn mark_all_interrupted(&self) -> Result<Vec<(String, i64)>, StoreError> {
        let conn = self.conn.lock().unwrap();
        let mut st = conn.prepare("SELECT session_id, id FROM messages WHERE streaming=1")?;
        let rows: Vec<(String, i64)> = st
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?
            .collect::<Result<Vec<_>, _>>()?;
        drop(st);
        conn.execute(
            "UPDATE messages SET streaming=0, interrupted=1 WHERE streaming=1",
            [],
        )?;
        conn.execute(
            "UPDATE sessions SET status='interrupted' WHERE id IN (SELECT DISTINCT session_id FROM messages WHERE interrupted=1)",
            [],
        )?;
        Ok(rows)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parts_parts_parts() {
        let s = Store::open_in_memory().unwrap();
        let sess = s.new_session("t", "m").unwrap();
        let mid = s.begin_message(&sess.id, "assistant", true).unwrap();
        s.put_part(&sess.id, mid, "text", "hel").unwrap();
        let seq = s.put_part(&sess.id, mid, "text", "hello world").unwrap();
        s.put_part(&sess.id, mid, "reasoning", "thinking...")
            .unwrap();
        s.finalize_message(&sess.id, mid, 12, false).unwrap();
        let t = s.transcript(&sess.id).unwrap();
        assert_eq!(t.len(), 1);
        // same-kind streaming parts coalesce into ONE row with the latest snapshot
        let kinds: Vec<&str> = t[0].1.iter().map(|p| p.kind.as_str()).collect();
        assert_eq!(kinds, vec!["text", "reasoning"]);
        let text = t[0].1.iter().find(|p| p.kind == "text").unwrap();
        assert_eq!(text.content, "hello world");
        let after = s.next_event_seq();
        assert!(after > seq);
        assert_eq!(s.interrupted_sessions().unwrap().len(), 0);
    }

    #[test]
    fn crash_recovery_marks_interrupted() {
        let s = Store::open_in_memory().unwrap();
        let sess = s.new_session("t", "m").unwrap();
        let mid = s.begin_message(&sess.id, "assistant", true).unwrap();
        s.put_part(&sess.id, mid, "text", "partial").unwrap();
        // simulate crash: new engine start on same file
        let affected = s.mark_all_interrupted().unwrap();
        assert_eq!(affected, vec![(sess.id.clone(), mid)]);
        assert_eq!(s.interrupted_sessions().unwrap().len(), 0);
        let t = s.transcript(&sess.id).unwrap();
        assert!(t[0].0.interrupted);
        assert_eq!(t[0].1[0].content, "partial");
        let listed = s.list_sessions().unwrap();
        assert_eq!(listed[0].status, "interrupted");
    }
}

#[cfg(test)]
mod m2_tests {
    use super::*;
    use std::io::Write;

    fn boot() -> (Store, StoredSession, i64) {
        let s = Store::open_in_memory().unwrap();
        let sess = s.new_session("t", "m").unwrap();
        let mid = s.begin_message(&sess.id, "assistant", true).unwrap();
        (s, sess, mid)
    }

    #[test]
    fn tool_call_lifecycle() {
        let (s, sess, mid) = boot();
        s.tool_call_started(&sess.id, mid, "call_1", "read", "{\"path\":\"a\"}")
            .unwrap();
        s.tool_call_finished("call_1", true, Some("1: hello"), None, 12)
            .unwrap();
        let calls = s.tool_calls_for(mid).unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].status, "done");
        assert_eq!(calls[0].output.as_deref(), Some("1: hello"));
        assert_eq!(calls[0].duration_ms, 12);
    }

    #[test]
    fn pending_calls_marked_interrupted_never_rerun() {
        let (s, sess, mid) = boot();
        s.tool_call_started(&sess.id, mid, "call_w", "write", "{}")
            .unwrap();
        let pend = s.pending_tool_calls().unwrap();
        assert_eq!(pend.len(), 1);
        s.mark_pending_tools_interrupted().unwrap();
        let calls = s.tool_calls_for(mid).unwrap();
        assert_eq!(calls[0].status, "interrupted");
        assert!(s.pending_tool_calls().unwrap().is_empty());
    }

    #[test]
    fn preimages_roundtrip() {
        let (s, sess, mid) = boot();
        s.tool_call_started(&sess.id, mid, "call_e", "edit", "{}")
            .unwrap();
        s.save_preimage("call_e", "src/a.rs", Some("fn a() {}"), None)
            .unwrap();
        s.save_preimage("call_e", "big.rs", None, Some("/tmp/spill/spill-1-x.txt"))
            .unwrap();
        let pre = s.preimages_for("call_e").unwrap();
        assert_eq!(pre.len(), 2);
        assert_eq!(pre[0].content.as_deref(), Some("fn a() {}"));
        assert_eq!(
            pre[1].spill_path.as_deref(),
            Some("/tmp/spill/spill-1-x.txt")
        );
    }

    #[test]
    fn db_size_and_gc() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.db");
        let s = Store::open(&path).unwrap();
        let size = s.db_size_bytes().unwrap();
        assert!(size > 0);
        let sess = s.new_session("g", "m").unwrap();
        let mid = s.begin_message(&sess.id, "assistant", true).unwrap();
        s.tool_call_started(&sess.id, mid, "c", "edit", "{}")
            .unwrap();
        s.save_preimage("c", "old.rs", Some("data"), None).unwrap();
        // retain nothing older than now+1ms -> deletes the preimage
        let (deleted, _vac) = s.gc(now_ms() + 1).unwrap();
        assert_eq!(deleted, 1);
        assert!(s.preimages_for("c").unwrap().is_empty());
        let mut f = std::fs::File::create(dir.path().join("marker")).unwrap();
        writeln!(f, "ok").unwrap();
    }
}
