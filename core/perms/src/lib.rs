//! palmcode-perms -- permission engine for M2/M3.
//!
//! Decision flow for (tool, path): first matching rule wins, in precedence
//! order — session approvals ("always for this session") → persisted
//! project approvals (app DB, "always for this project") → project
//! `<root>/.palmcode/config.json` → user-global config → built-in defaults.
//!
//! M3 trust model: an UNTRUSTED workspace's project config is ignored
//! completely (a hostile repo must not self-grant). A TRUSTED workspace's
//! project config may add ask/deny rules only — allow rules in it are
//! stripped at load. User-granted project allows live in the app DB keyed
//! by workspace id, never in repo files.
//!
//! Outside-workspace paths are rejected by RootSet before dispatch; an
//! explicit interactive approval adds a session root (agent-side), so this
//! module only ever sees granted paths.
//!
//! Invariant: tool output and file content NEVER become rules — rules come
//! from the app DB, disk configs and interactive approval only.

use std::path::Path;
use std::sync::Mutex;

use globset::{Glob, GlobSet, GlobSetBuilder};
use serde::{Deserialize, Serialize};

pub mod bash;

pub const TOOLS_READ: &[&str] = &["read", "glob", "grep", "ls"];
pub const TOOLS_WRITE: &[&str] = &["write", "edit", "apply_patch"];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decision {
    Allow,
    Ask,
    Deny,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuleCfg {
    /// Tool name or pipe-separated set, e.g. "write|edit|apply_patch", or "*".
    pub tool: String,
    /// Glob matched against the workspace-relative path (or absolute).
    pub path: String,
    pub decision: DecisionKind,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum DecisionKind {
    Allow,
    Ask,
    Deny,
}

impl From<DecisionKind> for Decision {
    fn from(k: DecisionKind) -> Self {
        match k {
            DecisionKind::Allow => Decision::Allow,
            DecisionKind::Ask => Decision::Ask,
            DecisionKind::Deny => Decision::Deny,
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ConfigFile {
    #[serde(default)]
    pub permissions: Vec<RuleCfg>,
}

struct CompiledRule {
    tools: Vec<String>,
    paths: GlobSet,
    decision: Decision,
}

struct Layer {
    rules: Vec<CompiledRule>,
}

fn compile(cfgs: &[RuleCfg]) -> std::result::Result<Layer, String> {
    let mut rules = Vec::new();
    for r in cfgs {
        let mut b = GlobSetBuilder::new();
        b.add(Glob::new(&r.path).map_err(|e| format!("glob '{}': {e}", r.path))?);
        rules.push(CompiledRule {
            tools: r.tool.split('|').map(|s| s.trim().to_string()).collect(),
            paths: b.build().map_err(|e| e.to_string())?,
            decision: r.decision.into(),
        });
    }
    Ok(Layer { rules })
}

impl Layer {
    fn check(&self, tool: &str, rel: &str, abs: &Path) -> Option<Decision> {
        for r in &self.rules {
            if (r.tools.iter().any(|t| t == "*" || t == tool))
                && (r.paths.is_match(rel) || r.paths.is_match(abs))
            {
                return Some(r.decision);
            }
        }
        None
    }
}

/// Secret-like basenames: reading them ships secrets to the provider.
fn secret_globset() -> GlobSet {
    let mut b = GlobSetBuilder::new();
    for g in ["**/.env", "**/.env.*", "**/*.pem", "**/id_rsa*", "**/*.key"] {
        b.add(Glob::new(g).unwrap());
    }
    b.build().unwrap()
}

/// `.git/**` anywhere in the tree.
fn gitdir_globset() -> GlobSet {
    let mut b = GlobSetBuilder::new();
    b.add(Glob::new("**/.git/**").unwrap());
    b.add(Glob::new(".git/**").unwrap());
    b.build().unwrap()
}

/// `.palmcode/**` anywhere — config/prompt-adjacent files; writes ask.
fn palmcode_dir_globset() -> GlobSet {
    let mut b = GlobSetBuilder::new();
    b.add(Glob::new("**/.palmcode/**").unwrap());
    b.add(Glob::new(".palmcode/**").unwrap());
    b.build().unwrap()
}

pub enum Grant {
    /// Single use.
    Once,
    /// Remembered for the session.
    Session,
    /// Persisted to the project config (agent-side writes the file).
    Project,
}

pub struct Engine {
    /// Persisted "always for project" approvals from the app DB.
    approvals: Layer,
    project: Layer,
    user: Layer,
    secrets: GlobSet,
    gitdir: GlobSet,
    palmcode_dir: GlobSet,
    session_rules: Mutex<Vec<(String, GlobSet)>>, // always-allow (tool, paths)
    accept_edits_session: Mutex<bool>,
    /// Bash command approvals: exact command strings, never globs (a `*` in
    /// a command must not broaden the match -- deliverable F).
    bash_project: std::collections::HashSet<String>,
    bash_session: Mutex<std::collections::HashSet<String>>,
}

impl Engine {
    /// `trusted`: untrusted workspaces get NO project-config rules at all;
    /// trusted workspaces get the config's ask/deny rules (allow stripped).
    /// `db_approvals`: (tool, path_glob) pairs from the app DB.
    pub fn load(
        workspace_root: &Path,
        user_cfg: Option<&str>,
        trusted: bool,
        db_approvals: &[(String, String)],
    ) -> std::result::Result<Self, String> {
        // bash command approvals are exact strings, kept out of the glob layers
        let (bash_db, path_db): (Vec<_>, Vec<_>) =
            db_approvals.iter().partition(|(t, _)| t == "bash");
        let approvals = {
            let cfgs: Vec<RuleCfg> = path_db
                .iter()
                .map(|(t, p)| RuleCfg {
                    tool: t.clone(),
                    path: p.clone(),
                    decision: DecisionKind::Allow,
                })
                .collect();
            compile(&cfgs)?
        };
        let project = if !trusted {
            compile(&[])?
        } else {
            let cfg_path = workspace_root.join(".palmcode/config.json");
            match std::fs::read_to_string(&cfg_path) {
                Ok(text) => {
                    let cf: ConfigFile =
                        serde_json::from_str(&text).map_err(|e| format!("{cfg_path:?}: {e}"))?;
                    let hardened: Vec<RuleCfg> = cf
                        .permissions
                        .into_iter()
                        .filter(|r| r.decision != DecisionKind::Allow)
                        .collect();
                    compile(&hardened)?
                }
                Err(_) => compile(&[])?,
            }
        };
        let user = match user_cfg {
            Some(text) => {
                let cf: ConfigFile =
                    serde_json::from_str(text).map_err(|e| format!("user config: {e}"))?;
                compile(&cf.permissions)?
            }
            None => compile(&[])?,
        };
        Ok(Self {
            approvals,
            project,
            user,
            secrets: secret_globset(),
            gitdir: gitdir_globset(),
            palmcode_dir: palmcode_dir_globset(),
            session_rules: Mutex::new(Vec::new()),
            accept_edits_session: Mutex::new(false),
            bash_project: bash_db.iter().map(|(_, p)| p.clone()).collect(),
            bash_session: Mutex::new(std::collections::HashSet::new()),
        })
    }

    pub fn set_accept_edits_for_session(&self) {
        *self.accept_edits_session.lock().unwrap() = true;
    }

    pub fn accept_edits_on(&self) -> bool {
        *self.accept_edits_session.lock().unwrap()
    }

    /// Remember an "always for session" approval.
    pub fn allow_for_session(
        &self,
        tool: &str,
        path_glob: &str,
    ) -> std::result::Result<(), String> {
        let mut b = GlobSetBuilder::new();
        b.add(Glob::new(path_glob).map_err(|e| e.to_string())?);
        let set = b.build().map_err(|e| e.to_string())?;
        self.session_rules
            .lock()
            .unwrap()
            .push((tool.to_string(), set));
        Ok(())
    }

    /// Bash commands (deliverable F): exact-match allow cache first, then the
    /// classifier. Approved commands are matched as literal strings.
    pub fn decide_bash(&self, command: &str, ctx: &bash::ClassCtx) -> bash::Outcome {
        if self.bash_session.lock().unwrap().contains(command)
            || self.bash_project.contains(command)
        {
            return bash::Outcome::Allow;
        }
        bash::classify(command, ctx)
    }

    /// Remember an approved bash command for this session (exact string).
    pub fn allow_bash_for_session(&self, command: &str) {
        self.bash_session
            .lock()
            .unwrap()
            .insert(command.to_string());
    }

    /// The decision for (tool, rel, abs). rel is the workspace-relative path
    /// with forward slashes.
    pub fn decide(&self, tool: &str, rel: &str, abs: &Path) -> Decision {
        // session approvals
        {
            let rules = self.session_rules.lock().unwrap();
            for (t, set) in rules.iter() {
                if (t == "*" || t == tool) && (set.is_match(rel) || set.is_match(abs)) {
                    return Decision::Allow;
                }
            }
        }
        // persisted project approvals (app DB), then project config, then user
        if let Some(d) = self.approvals.check(tool, rel, abs) {
            return d;
        }
        if let Some(d) = self.project.check(tool, rel, abs) {
            return d;
        }
        if let Some(d) = self.user.check(tool, rel, abs) {
            return d;
        }
        // built-ins
        if TOOLS_WRITE.contains(&tool) && (self.gitdir.is_match(rel) || self.gitdir.is_match(abs)) {
            return Decision::Deny;
        }
        if TOOLS_WRITE.contains(&tool)
            && (self.palmcode_dir.is_match(rel) || self.palmcode_dir.is_match(abs))
        {
            // config-adjacent files (permissions, future hook config): the
            // model may write them, but never silently
            return Decision::Ask;
        }
        if TOOLS_READ.contains(&tool) && (self.secrets.is_match(rel) || self.secrets.is_match(abs))
        {
            return Decision::Ask;
        }
        if TOOLS_READ.contains(&tool) {
            return Decision::Allow;
        }
        if TOOLS_WRITE.contains(&tool) {
            if self.accept_edits_on() {
                return Decision::Allow;
            }
            return Decision::Ask;
        }
        Decision::Ask
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup(cfg: Option<&str>, trusted: bool) -> (tempfile::TempDir, Engine) {
        let dir = tempfile::tempdir().unwrap();
        if let Some(c) = cfg {
            let d = dir.path().join(".palmcode");
            std::fs::create_dir_all(&d).unwrap();
            std::fs::write(d.join("config.json"), c).unwrap();
        }
        let e = Engine::load(dir.path(), None, trusted, &[]).unwrap();
        (dir, e)
    }

    #[test]
    fn defaults_reads_allow_writes_ask() {
        let (d, e) = setup(None, false);
        let abs = d.path().join("src/a.rs");
        assert_eq!(e.decide("read", "src/a.rs", &abs), Decision::Allow);
        assert_eq!(e.decide("grep", "src/a.rs", &abs), Decision::Allow);
        assert_eq!(e.decide("write", "src/a.rs", &abs), Decision::Ask);
        assert_eq!(e.decide("edit", "src/a.rs", &abs), Decision::Ask);
        assert_eq!(e.decide("apply_patch", "src/a.rs", &abs), Decision::Ask);
    }

    #[test]
    fn defaults_git_deny_and_secrets_ask() {
        let (d, e) = setup(None, false);
        assert_eq!(
            e.decide("write", ".git/config", &d.path().join(".git/config")),
            Decision::Deny
        );
        assert_eq!(
            e.decide("edit", "sub/.git/index", &d.path().join("sub/.git/index")),
            Decision::Deny
        );
        for p in [
            ".env",
            ".env.local",
            "keys/a.pem",
            "id_rsa",
            "tls/server.key",
        ] {
            assert_eq!(
                e.decide("read", p, &d.path().join(p)),
                Decision::Ask,
                "secret read not ask: {p}"
            );
        }
        // unknown tools default to Ask
        assert_eq!(
            e.decide("webfetch", "x", &d.path().join("x")),
            Decision::Ask
        );
    }

    #[test]
    fn trusted_project_keeps_deny_ask_strips_allow() {
        let (d, e) = setup(
            Some(
                r#"{ "permissions": [
                { "tool": "write|edit", "path": "docs/**", "decision": "allow" },
                { "tool": "read", "path": ".env", "decision": "deny" }
            ] }"#,
            ),
            true,
        );
        // allow rules are stripped even for trusted projects...
        assert_eq!(
            e.decide("write", "docs/readme.md", &d.path().join("docs/readme.md")),
            Decision::Ask
        );
        assert_eq!(
            e.decide("edit", "docs/readme.md", &d.path().join("docs/readme.md")),
            Decision::Ask
        );
        // ...but deny/ask rules are honored (deny overrides builtin ask)
        assert_eq!(
            e.decide("read", ".env", &d.path().join(".env")),
            Decision::Deny
        );
        // untouched defaults
        assert_eq!(
            e.decide("write", "src/a.rs", &d.path().join("src/a.rs")),
            Decision::Ask
        );
    }

    #[test]
    fn untrusted_ignores_project_config_completely() {
        // hostile repo: allow-self-grant in .palmcode/config.json must do
        // nothing, deny rules too — the whole file is ignored
        let (d, e) = setup(
            Some(
                r#"{ "permissions": [
                { "tool": "write", "path": "**", "decision": "allow" },
                { "tool": "read", "path": "docs/**", "decision": "deny" }
            ] }"#,
            ),
            false,
        );
        assert_eq!(
            e.decide("write", "src/a.rs", &d.path().join("src/a.rs")),
            Decision::Ask
        );
        assert_eq!(
            e.decide("read", "docs/a.md", &d.path().join("docs/a.md")),
            Decision::Allow
        );
    }

    #[test]
    fn db_project_approvals_allow() {
        let dir = tempfile::tempdir().unwrap();
        let e = Engine::load(
            dir.path(),
            None,
            false,
            &[("edit".into(), "target/generated/**".into())],
        )
        .unwrap();
        assert_eq!(
            e.decide(
                "edit",
                "target/generated/g.rs",
                &dir.path().join("target/generated/g.rs")
            ),
            Decision::Allow
        );
    }

    #[test]
    fn first_matching_rule_wins_within_layer() {
        let (d, e) = setup(
            Some(
                r#"{ "permissions": [
                { "tool": "write", "path": "docs/private/**", "decision": "deny" },
                { "tool": "write", "path": "docs/**", "decision": "ask" }
            ] }"#,
            ),
            true,
        );
        assert_eq!(
            e.decide("write", "docs/private/x", &d.path().join("docs/private/x")),
            Decision::Deny
        );
        assert_eq!(
            e.decide("write", "docs/pub/x", &d.path().join("docs/pub/x")),
            Decision::Ask
        );
    }

    #[test]
    fn session_grant_and_auto_accept() {
        let (d, e) = setup(None, false);
        e.allow_for_session("write", "src/gen/**").unwrap();
        assert_eq!(
            e.decide("write", "src/gen/a.rs", &d.path().join("src/gen/a.rs")),
            Decision::Allow
        );
        assert_eq!(
            e.decide("write", "src/other.rs", &d.path().join("src/other.rs")),
            Decision::Ask
        );
        e.set_accept_edits_for_session();
        assert_eq!(
            e.decide("write", "src/other.rs", &d.path().join("src/other.rs")),
            Decision::Allow
        );
        // secrets still ask, deny still deny
        assert_eq!(
            e.decide("read", ".env", &d.path().join(".env")),
            Decision::Ask
        );
        assert_eq!(
            e.decide("write", ".git/config", &d.path().join(".git/config")),
            Decision::Deny
        );
    }

    #[test]
    fn palmcode_dir_writes_always_ask() {
        let (d, e) = setup(None, false);
        for p in [".palmcode/config.json", "sub/.palmcode/hooks.json"] {
            assert_eq!(
                e.decide("write", p, &d.path().join(p)),
                Decision::Ask,
                "{p} must ask"
            );
        }
        // reads stay free
        let abs = d.path().join(".palmcode/config.json");
        assert_eq!(
            e.decide("read", ".palmcode/config.json", &abs),
            Decision::Allow
        );
    }

    #[test]
    fn malformed_config_is_error_not_allow() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path().join(".palmcode");
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("config.json"), "{ not json").unwrap();
        // trusted: malformed config errors (fail closed), never loads
        assert!(Engine::load(dir.path(), None, true, &[]).is_err());
        // malformed glob also errors at load
        std::fs::write(
            d.join("config.json"),
            r#"{ "permissions": [ {"tool":"*","path":"[bad","decision":"deny"} ] }"#,
        )
        .unwrap();
        assert!(Engine::load(dir.path(), None, true, &[]).is_err());
        // untrusted: the same garbage is simply ignored
        assert!(Engine::load(dir.path(), None, false, &[]).is_ok());
    }
}
