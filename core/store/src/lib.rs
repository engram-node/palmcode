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
";

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
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
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    pub fn open_in_memory() -> Result<Self, StoreError> {
        let conn = Connection::open_in_memory()?;
        conn.execute_batch(SCHEMA)?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
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
