use super::*;
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::thread::JoinHandle;

const QUEUE_CAPACITY: usize = 64;
const BATCH_LIMIT: usize = 32;
type Completion = Box<dyn FnOnce(Option<String>) + Send>;
type Operation = Box<dyn FnOnce(&DbStore) -> (bool, Completion) + Send>;

struct Job {
    write: bool,
    run: Operation,
}

/// One connection owned exclusively by a bounded FIFO writer worker.
pub struct Store {
    sender: Option<SyncSender<Job>>,
    worker: Option<JoinHandle<()>>,
}

impl Store {
    pub fn open(path: &Path) -> Result<Self, StoreError> {
        let path = path.to_owned();
        Self::start(move || DbStore::open(&path))
    }

    pub fn open_in_memory() -> Result<Self, StoreError> {
        Self::start(DbStore::open_in_memory)
    }

    fn start(
        open: impl FnOnce() -> Result<DbStore, StoreError> + Send + 'static,
    ) -> Result<Self, StoreError> {
        let (sender, receiver) = mpsc::sync_channel(QUEUE_CAPACITY);
        let (ready, startup) = mpsc::sync_channel(1);
        let worker = std::thread::Builder::new()
            .name("palmcode-store".into())
            .spawn(move || match open() {
                Ok(db) => {
                    let _ = ready.send(Ok(()));
                    run_writer(db, receiver);
                }
                Err(error) => {
                    let _ = ready.send(Err(error));
                }
            })?;
        startup
            .recv()
            .map_err(|_| StoreError::Writer("startup disconnected".into()))??;
        Ok(Self {
            sender: Some(sender),
            worker: Some(worker),
        })
    }

    fn call<T: Send + 'static>(
        &self,
        write: bool,
        operation: impl FnOnce(&DbStore) -> Result<T, StoreError> + Send + 'static,
    ) -> Result<T, StoreError> {
        let _phase = palmcode_telemetry::Phase::start("persist");
        let (reply, receive) = mpsc::sync_channel(1);
        self.submit(Job {
            write,
            run: Box::new(move |db| {
                let result = operation(db);
                (
                    result.is_err(),
                    Box::new(move |failure| {
                        let _ = reply.send(match failure {
                            Some(reason) => Err(StoreError::Writer(reason)),
                            None => result,
                        });
                    }),
                )
            }),
        })?;
        receive
            .recv()
            .map_err(|_| StoreError::Writer("worker disconnected".into()))?
    }

    fn submit(&self, job: Job) -> Result<(), StoreError> {
        self.sender
            .as_ref()
            .ok_or_else(|| StoreError::Writer("closed".into()))?
            .send(job)
            .map_err(|_| StoreError::Writer("worker disconnected".into()))
    }

    /// Enqueue a cumulative snapshot. Finalization/read/flush is a commit barrier.
    /// A full queue backpressures the producer; a failed batch poisons the worker.
    pub fn enqueue_part(
        &self,
        session: &str,
        message_id: i64,
        kind: &str,
        content: &str,
    ) -> Result<(), StoreError> {
        let (session, kind, content) = (session.to_owned(), kind.to_owned(), content.to_owned());
        self.submit(Job {
            write: true,
            run: Box::new(move |db| {
                let failed = db.put_part(&session, message_id, &kind, &content).is_err();
                (failed, Box::new(|_| {}))
            }),
        })
    }

    pub fn flush(&self) -> Result<(), StoreError> {
        self.call(false, |_| Ok(()))
    }

    /// One durable batch before any tool in the exchange may execute.
    pub fn tool_calls_started(
        &self,
        session: &str,
        message_id: i64,
        calls: &[(String, String, String)],
    ) -> Result<(), StoreError> {
        let session = session.to_owned();
        let calls = calls.to_owned();
        self.call(true, move |db| {
            for (id, name, args) in calls {
                db.tool_call_started(&session, message_id, &id, &name, &args)?;
            }
            Ok(())
        })
    }
}

impl Drop for Store {
    fn drop(&mut self) {
        self.sender.take();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

fn run_writer(db: DbStore, receiver: Receiver<Job>) {
    let mut pending = None;
    let mut failure: Option<String> = None;
    loop {
        let Some(first) = pending.take().or_else(|| receiver.recv().ok()) else {
            break;
        };
        if let Some(reason) = &failure {
            // Do not execute more SQL after an unacknowledged/rolled-back batch.
            // Complete through a separate rejection path without running jobs.
            reject(first, reason);
            continue;
        }
        if !first.write {
            let (_, complete) = (first.run)(&db);
            complete(None);
            continue;
        }
        let mut jobs = vec![first];
        while jobs.len() < BATCH_LIMIT {
            match receiver.try_recv() {
                Ok(job) if job.write => jobs.push(job),
                Ok(job) => {
                    pending = Some(job);
                    break;
                }
                Err(_) => break,
            }
        }
        let begin = db.conn.lock().unwrap().execute_batch("BEGIN IMMEDIATE");
        if begin.is_err() {
            failure = Some("begin failed; reopen required".into());
            for job in jobs {
                reject(job, failure.as_deref().unwrap());
            }
            continue;
        }
        let started = std::time::Instant::now();
        let count = jobs.len();
        let mut completions = Vec::new();
        let mut failed = false;
        for job in jobs {
            let (bad, complete) = (job.run)(&db);
            failed |= bad;
            completions.push(complete);
        }
        if failed {
            let _ = db.conn.lock().unwrap().execute_batch("ROLLBACK");
            failure = Some("batch rolled back; reopen required".into());
        } else if db.conn.lock().unwrap().execute_batch("COMMIT").is_err() {
            let _ = db.conn.lock().unwrap().execute_batch("ROLLBACK");
            failure = Some("commit failed; reopen required".into());
        }
        palmcode_telemetry::alog(&format!(
            "PERSIST_BATCH count={count} us={}",
            started.elapsed().as_micros()
        ));
        for complete in completions {
            complete(failure.clone());
        }
    }
}

fn reject(job: Job, reason: &str) {
    // Rejection must not evaluate a queued operation. Its completion sender is
    // dropped with the job; synchronous callers receive disconnection, never OK.
    let _ = reason;
    drop(job);
}

impl Store {
    pub fn meta_get(&self, key: &str) -> Result<Option<String>, StoreError> {
        let key = key.to_owned();
        self.call(false, move |db| db.meta_get(&key))
    }
    pub fn meta_set(&self, key: &str, value: &str) -> Result<(), StoreError> {
        let key = key.to_owned();
        let value = value.to_owned();
        self.call(true, move |db| db.meta_set(&key, &value))
    }
    pub fn next_event_seq(&self) -> i64 {
        self.call(false, move |db| Ok(db.next_event_seq()))
            .unwrap_or(1)
    }
    pub fn new_session(&self, title: &str, model: &str) -> Result<StoredSession, StoreError> {
        let title = title.to_owned();
        let model = model.to_owned();
        self.call(true, move |db| db.new_session(&title, &model))
    }
    pub fn list_sessions(&self) -> Result<Vec<StoredSession>, StoreError> {
        self.call(false, move |db| db.list_sessions())
    }
    pub fn begin_message(
        &self,
        session: &str,
        role: &str,
        streaming: bool,
    ) -> Result<i64, StoreError> {
        let session = session.to_owned();
        let role = role.to_owned();
        self.call(true, move |db| db.begin_message(&session, &role, streaming))
    }
    pub fn put_part(
        &self,
        session: &str,
        message_id: i64,
        kind: &str,
        content: &str,
    ) -> Result<i64, StoreError> {
        let session = session.to_owned();
        let kind = kind.to_owned();
        let content = content.to_owned();
        self.call(true, move |db| {
            db.put_part(&session, message_id, &kind, &content)
        })
    }
    pub fn finalize_message(
        &self,
        session: &str,
        message_id: i64,
        tokens: u32,
        interrupted: bool,
    ) -> Result<(), StoreError> {
        let session = session.to_owned();
        self.call(true, move |db| {
            db.finalize_message(&session, message_id, tokens, interrupted)
        })
    }
    pub fn workspace_upsert(
        &self,
        name: &str,
        root_path: &str,
    ) -> Result<StoredWorkspace, StoreError> {
        let name = name.to_owned();
        let root_path = root_path.to_owned();
        self.call(true, move |db| db.workspace_upsert(&name, &root_path))
    }
    pub fn workspaces_list(&self) -> Result<Vec<StoredWorkspace>, StoreError> {
        self.call(false, move |db| db.workspaces_list())
    }
    pub fn workspace_set_trusted(&self, id: &str, trusted: bool) -> Result<(), StoreError> {
        let id = id.to_owned();
        self.call(true, move |db| db.workspace_set_trusted(&id, trusted))
    }
    pub fn project_approvals_list(
        &self,
        workspace_id: &str,
    ) -> Result<Vec<(String, String)>, StoreError> {
        let workspace_id = workspace_id.to_owned();
        self.call(false, move |db| db.project_approvals_list(&workspace_id))
    }
    pub fn project_approval_add(
        &self,
        workspace_id: &str,
        tool: &str,
        path_glob: &str,
    ) -> Result<(), StoreError> {
        let workspace_id = workspace_id.to_owned();
        let tool = tool.to_owned();
        let path_glob = path_glob.to_owned();
        self.call(true, move |db| {
            db.project_approval_add(&workspace_id, &tool, &path_glob)
        })
    }
    pub fn workspace_touch_by_root(&self, root_path: &str) -> Result<(), StoreError> {
        let root_path = root_path.to_owned();
        self.call(true, move |db| db.workspace_touch_by_root(&root_path))
    }
    pub fn workspace_remove(&self, id: &str) -> Result<Option<String>, StoreError> {
        let id = id.to_owned();
        self.call(true, move |db| db.workspace_remove(&id))
    }
    pub fn set_status(&self, session: &str, status: &str) -> Result<(), StoreError> {
        let session = session.to_owned();
        let status = status.to_owned();
        self.call(true, move |db| db.set_status(&session, &status))
    }
    pub fn transcript(
        &self,
        session: &str,
    ) -> Result<Vec<(StoredMessage, Vec<StoredPart>)>, StoreError> {
        let session = session.to_owned();
        self.call(false, move |db| db.transcript(&session))
    }
    pub fn interrupted_sessions(&self) -> Result<Vec<String>, StoreError> {
        self.call(false, move |db| db.interrupted_sessions())
    }
    pub fn tool_call_started(
        &self,
        session: &str,
        message_id: i64,
        call_id: &str,
        name: &str,
        args: &str,
    ) -> Result<(), StoreError> {
        let session = session.to_owned();
        let call_id = call_id.to_owned();
        let name = name.to_owned();
        let args = args.to_owned();
        self.call(true, move |db| {
            db.tool_call_started(&session, message_id, &call_id, &name, &args)
        })
    }
    pub fn tool_call_finished(
        &self,
        call_id: &str,
        ok: bool,
        output: Option<&str>,
        spill_path: Option<&str>,
        duration_ms: i64,
    ) -> Result<(), StoreError> {
        let call_id = call_id.to_owned();
        let output = output.map(str::to_owned);
        let spill_path = spill_path.map(str::to_owned);
        self.call(true, move |db| {
            db.tool_call_finished(
                &call_id,
                ok,
                output.as_deref(),
                spill_path.as_deref(),
                duration_ms,
            )
        })
    }
    pub fn tool_calls_for(&self, message_id: i64) -> Result<Vec<StoredToolCall>, StoreError> {
        self.call(false, move |db| db.tool_calls_for(message_id))
    }
    pub fn pending_tool_calls(&self) -> Result<Vec<StoredToolCall>, StoreError> {
        self.call(false, move |db| db.pending_tool_calls())
    }
    pub fn mark_pending_tools_interrupted(&self) -> Result<(), StoreError> {
        self.call(true, move |db| db.mark_pending_tools_interrupted())
    }
    pub fn save_preimage(
        &self,
        call_id: &str,
        path: &str,
        content: Option<&str>,
        spill_path: Option<&str>,
    ) -> Result<(), StoreError> {
        let call_id = call_id.to_owned();
        let path = path.to_owned();
        let content = content.map(str::to_owned);
        let spill_path = spill_path.map(str::to_owned);
        self.call(true, move |db| {
            db.save_preimage(&call_id, &path, content.as_deref(), spill_path.as_deref())
        })
    }
    pub fn preimages_for(&self, call_id: &str) -> Result<Vec<Preimage>, StoreError> {
        let call_id = call_id.to_owned();
        self.call(false, move |db| db.preimages_for(&call_id))
    }
    pub fn db_size_bytes(&self) -> Result<u64, StoreError> {
        self.call(false, move |db| db.db_size_bytes())
    }
    pub fn gc(&self, retained_after_ms: i64) -> Result<(u64, bool), StoreError> {
        self.call(false, move |db| db.gc(retained_after_ms))
    }
    pub fn mark_all_interrupted(&self) -> Result<Vec<(String, i64)>, StoreError> {
        self.call(true, move |db| db.mark_all_interrupted())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn gate(store: &Store) -> SyncSender<()> {
        let (entered, ready) = mpsc::sync_channel(1);
        let (release, wait) = mpsc::sync_channel(1);
        store
            .submit(Job {
                write: false,
                run: Box::new(move |_| {
                    entered.send(()).unwrap();
                    wait.recv().unwrap();
                    (false, Box::new(|_| {}))
                }),
            })
            .unwrap();
        ready.recv_timeout(Duration::from_secs(2)).unwrap();
        release
    }

    #[test]
    fn queued_snapshots_are_ordered_and_drop_drains_before_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("store.db");
        let session;
        let mid;
        {
            let store = Store::open(&path).unwrap();
            session = store.new_session("test", "").unwrap().id;
            mid = store.begin_message(&session, "assistant", true).unwrap();
            for n in 0..100 {
                store
                    .enqueue_part(&session, mid, "text", &n.to_string())
                    .unwrap();
            }
        }
        let store = Store::open(&path).unwrap();
        let rows = store.transcript(&session).unwrap();
        assert_eq!(rows[0].1[0].content, "99");
        assert_eq!(
            store.mark_all_interrupted().unwrap(),
            vec![(session.clone(), mid)]
        );
        let payloads = store
            .call(false, |db| {
                let conn = db.conn.lock().unwrap();
                let mut statement =
                    conn.prepare("SELECT payload FROM events WHERE kind='part' ORDER BY seq")?;
                Ok(statement
                    .query_map([], |r| r.get::<_, String>(0))?
                    .collect::<Result<Vec<_>, _>>()?)
            })
            .unwrap();
        for (n, payload) in payloads.iter().enumerate() {
            assert_eq!(
                serde_json::from_str::<serde_json::Value>(payload).unwrap()["content"],
                n.to_string()
            );
        }
    }

    #[test]
    fn failed_batch_rolls_back_all_projections_and_poison_is_visible() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("store.db");
        let store = Store::open(&path).unwrap();
        let session = store.new_session("test", "").unwrap().id;
        let mid = store.begin_message(&session, "assistant", true).unwrap();
        store.put_part(&session, mid, "text", "before").unwrap();
        let release = gate(&store);
        store
            .enqueue_part(&session, mid, "text", "uncommitted")
            .unwrap();
        store
            .enqueue_part(&session, i64::MAX, "text", "invalid foreign key")
            .unwrap();
        release.send(()).unwrap();
        assert!(store.flush().is_err());
        assert!(store.meta_set("must-not-run", "value").is_err());
        drop(store);
        let store = Store::open(&path).unwrap();
        assert_eq!(
            store.transcript(&session).unwrap()[0].1[0].content,
            "before"
        );
        assert_eq!(store.meta_get("must-not-run").unwrap(), None);
    }

    #[test]
    fn full_queue_backpressures_and_sql_runs_on_the_worker() {
        let store = Store::open_in_memory().unwrap();
        let worker = store
            .call(false, |_| Ok(std::thread::current().id()))
            .unwrap();
        assert_ne!(worker, std::thread::current().id());
        let session = store.new_session("test", "").unwrap().id;
        let mid = store.begin_message(&session, "assistant", true).unwrap();
        let release = gate(&store);
        for _ in 0..QUEUE_CAPACITY {
            store
                .enqueue_part(&session, mid, "text", "snapshot")
                .unwrap();
        }
        std::thread::scope(|scope| {
            let (sent, received) = mpsc::sync_channel(1);
            let session = &session;
            let store = &store;
            scope.spawn(move || {
                store.enqueue_part(session, mid, "text", "last").unwrap();
                sent.send(()).unwrap();
            });
            assert!(received.recv_timeout(Duration::from_millis(100)).is_err());
            release.send(()).unwrap();
            received.recv_timeout(Duration::from_secs(2)).unwrap();
        });
        store.finalize_message(&session, mid, 0, false).unwrap();
        assert_eq!(store.transcript(&session).unwrap()[0].1[0].content, "last");
    }

    #[test]
    fn file_pragmas_preserve_full_wal_durability() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("store.db")).unwrap();
        let config = store
            .call(false, |db| {
                let conn = db.conn.lock().unwrap();
                Ok((
                    conn.query_row("PRAGMA journal_mode", [], |r| r.get::<_, String>(0))?,
                    conn.query_row("PRAGMA synchronous", [], |r| r.get::<_, i64>(0))?,
                    conn.query_row("PRAGMA foreign_keys", [], |r| r.get::<_, i64>(0))?,
                ))
            })
            .unwrap();
        assert_eq!(config, ("wal".into(), 2, 1));
    }
}
