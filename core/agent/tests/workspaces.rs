//! M2 E: workspace registry, metrics, safe removal.

use std::sync::Mutex;

use palmcode_agent::{Engine, Event, EventSink};

struct Sink(Mutex<Vec<Event>>);
impl EventSink for Sink {
    fn on_batch(&self, events: Vec<Event>) {
        self.0.lock().unwrap().extend(events);
    }
}

fn rig() -> (tempfile::TempDir, Engine) {
    let dir = tempfile::tempdir().unwrap();
    let engine = Engine::start(
        &dir.path().join("t.db"),
        std::sync::Arc::new(Sink(Mutex::new(vec![]))),
    )
    .unwrap();
    (dir, engine)
}

#[test]
fn register_list_metrics_remove() {
    let (_d, engine) = rig();
    let ws = tempfile::tempdir().unwrap();
    std::fs::write(ws.path().join("a.rs"), "fn a() {}\n").unwrap();
    std::fs::create_dir_all(ws.path().join("sub")).unwrap();
    std::fs::write(ws.path().join("sub/b.rs"), "fn b() {}\n").unwrap();
    std::fs::write(ws.path().join("skip.log"), "noise\n").unwrap();
    std::fs::write(ws.path().join(".gitignore"), "*.log\n").unwrap();
    let root = ws.path().to_str().unwrap();

    let w = engine.workspace_register("demo", root).unwrap();
    assert_eq!(w.name, "demo");
    // upsert of same root updates instead of duplicating
    let w2 = engine.workspace_register("demo2", root).unwrap();
    assert_eq!(w.id, w2.id);
    assert_eq!(w2.name, "demo2");

    let list = engine.workspaces_list().unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].root_path, root);

    let m = engine.workspace_metrics(root).unwrap();
    assert!(m.present);
    assert_eq!(m.file_count, 3, ".gitignore'd log excluded"); // a.rs, sub/b.rs, .gitignore
    assert_eq!(m.size_bytes, 10 + 10 + 6);

    let gone = engine.workspace_metrics("nonexistent-dir-xyz").unwrap();
    assert!(!gone.present);

    // remove without file deletion keeps user files
    engine.workspace_remove(&w.id, false).unwrap();
    assert!(engine.workspaces_list().unwrap().is_empty());
    assert!(ws.path().join("a.rs").exists());
}

#[test]
fn set_workspace_bumps_last_used() {
    let (_d, engine) = rig();
    let ws = tempfile::tempdir().unwrap();
    let root = ws.path().to_str().unwrap();
    let w = engine.workspace_register("demo", root).unwrap();
    engine.set_workspace(root).unwrap();
    let after = engine.workspaces_list().unwrap();
    assert!(after[0].last_used_at >= w.last_used_at);
}

#[test]
fn delete_files_only_inside_import_dir() {
    let (_d, engine) = rig();

    // private copy under the import dir: files may be deleted
    let import = engine.workspace_import_dir().join("copy1");
    std::fs::create_dir_all(&import).unwrap();
    std::fs::write(import.join("x.txt"), "x").unwrap();
    let w1 = engine
        .workspace_register("copy", import.to_str().unwrap())
        .unwrap();
    engine.workspace_remove(&w1.id, true).unwrap();
    assert!(!import.exists(), "private copy deleted on request");

    // user dir elsewhere: row goes, files stay even with delete_files=true
    let ws = tempfile::tempdir().unwrap();
    std::fs::write(ws.path().join("keep.txt"), "keep").unwrap();
    let w2 = engine
        .workspace_register("user", ws.path().to_str().unwrap())
        .unwrap();
    engine.workspace_remove(&w2.id, true).unwrap();
    assert!(ws.path().join("keep.txt").exists());
    assert!(engine.workspaces_list().unwrap().is_empty());
}

#[test]
fn removing_active_root_clears_the_workspace() {
    let (_d, engine) = rig();
    let ws = tempfile::tempdir().unwrap();
    let root = ws.path().to_str().unwrap();
    let w = engine.workspace_register("demo", root).unwrap();
    engine.set_workspace(root).unwrap();
    assert_eq!(engine.workspace().as_deref(), Some(root));
    engine.workspace_remove(&w.id, false).unwrap();
    assert_eq!(engine.workspace(), None);
}
