//! palmcode-perms -- permission engine for M2.
//!
//! Decision flow for (tool, path): first matching rule wins, in precedence
//! order — session approvals ("always for this session") → project
//! `<root>/.palmcode/config.json` → user-global config → built-in defaults.
//!
//! Outside-workspace paths are rejected by RootSet before dispatch; an
//! explicit interactive approval adds a session root (agent-side), so this
//! module only ever sees granted paths.
//!
//! Invariant: tool output and file content NEVER become rules — rules come
//! from disk configs and interactive approval only.

use std::path::Path;
use std::sync::Mutex;

use globset::{Glob, GlobSet, GlobSetBuilder};
use serde::{Deserialize, Serialize};

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

pub enum Grant {
    /// Single use.
    Once,
    /// Remembered for the session.
    Session,
    /// Persisted to the project config (agent-side writes the file).
    Project,
}

pub struct Engine {
    project: Layer,
    user: Layer,
    secrets: GlobSet,
    gitdir: GlobSet,
    session_rules: Mutex<Vec<(String, GlobSet)>>, // always-allow (tool, paths)
    accept_edits_session: Mutex<bool>,
}

impl Engine {
    pub fn load(
        workspace_root: &Path,
        user_cfg: Option<&str>,
    ) -> std::result::Result<Self, String> {
        let project = {
            let cfg_path = workspace_root.join(".palmcode/config.json");
            match std::fs::read_to_string(&cfg_path) {
                Ok(text) => {
                    let cf: ConfigFile =
                        serde_json::from_str(&text).map_err(|e| format!("{cfg_path:?}: {e}"))?;
                    compile(&cf.permissions)?
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
            project,
            user,
            secrets: secret_globset(),
            gitdir: gitdir_globset(),
            session_rules: Mutex::new(Vec::new()),
            accept_edits_session: Mutex::new(false),
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

    /// "Accept edits" + a Project grant are persisted by appending an allow
    /// rule to `<root>/.palmcode/config.json`.
    pub fn persist_project_allow(workspace_root: &Path, rule: RuleCfg) -> std::io::Result<()> {
        let dir = workspace_root.join(".palmcode");
        std::fs::create_dir_all(&dir)?;
        let cfg_path = dir.join("config.json");
        let mut cf: ConfigFile = std::fs::read_to_string(&cfg_path)
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or(ConfigFile {
                permissions: Vec::new(),
            });
        cf.permissions.push(rule);
        let text = serde_json::to_string_pretty(&cf).unwrap();
        std::fs::write(&cfg_path, text)
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
        // project, then user
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

    fn setup(cfg: Option<&str>) -> (tempfile::TempDir, Engine) {
        let dir = tempfile::tempdir().unwrap();
        if let Some(c) = cfg {
            let d = dir.path().join(".palmcode");
            std::fs::create_dir_all(&d).unwrap();
            std::fs::write(d.join("config.json"), c).unwrap();
        }
        let e = Engine::load(dir.path(), None).unwrap();
        (dir, e)
    }

    #[test]
    fn defaults_reads_allow_writes_ask() {
        let (d, e) = setup(None);
        let abs = d.path().join("src/a.rs");
        assert_eq!(e.decide("read", "src/a.rs", &abs), Decision::Allow);
        assert_eq!(e.decide("grep", "src/a.rs", &abs), Decision::Allow);
        assert_eq!(e.decide("write", "src/a.rs", &abs), Decision::Ask);
        assert_eq!(e.decide("edit", "src/a.rs", &abs), Decision::Ask);
        assert_eq!(e.decide("apply_patch", "src/a.rs", &abs), Decision::Ask);
    }

    #[test]
    fn defaults_git_deny_and_secrets_ask() {
        let (d, e) = setup(None);
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
    fn project_overrides_builtin_and_user() {
        let (d, e) = setup(Some(
            r#"{ "permissions": [
                { "tool": "write|edit", "path": "docs/**", "decision": "allow" },
                { "tool": "read", "path": ".env", "decision": "deny" }
            ] }"#,
        ));
        assert_eq!(
            e.decide("write", "docs/readme.md", &d.path().join("docs/readme.md")),
            Decision::Allow
        );
        assert_eq!(
            e.decide("edit", "docs/readme.md", &d.path().join("docs/readme.md")),
            Decision::Allow
        );
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
    fn first_matching_rule_wins_within_layer() {
        let (d, e) = setup(Some(
            r#"{ "permissions": [
                { "tool": "write", "path": "docs/private/**", "decision": "deny" },
                { "tool": "write", "path": "docs/**", "decision": "allow" }
            ] }"#,
        ));
        assert_eq!(
            e.decide("write", "docs/private/x", &d.path().join("docs/private/x")),
            Decision::Deny
        );
        assert_eq!(
            e.decide("write", "docs/pub/x", &d.path().join("docs/pub/x")),
            Decision::Allow
        );
    }

    #[test]
    fn session_grant_and_auto_accept() {
        let (d, e) = setup(None);
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
    fn persist_project_allow_roundtrip() {
        let (d, _e) = setup(None);
        Engine::persist_project_allow(
            d.path(),
            RuleCfg {
                tool: "edit".into(),
                path: "target/generated/**".into(),
                decision: DecisionKind::Allow,
            },
        )
        .unwrap();
        let e2 = Engine::load(d.path(), None).unwrap();
        assert_eq!(
            e2.decide(
                "edit",
                "target/generated/g.rs",
                &d.path().join("target/generated/g.rs")
            ),
            Decision::Allow
        );
    }

    #[test]
    fn malformed_config_is_error_not_allow() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path().join(".palmcode");
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("config.json"), "{ not json").unwrap();
        assert!(Engine::load(dir.path(), None).is_err());
        // malformed glob also errors at load
        std::fs::write(
            d.join("config.json"),
            r#"{ "permissions": [ {"tool":"*","path":"[bad","decision":"allow"} ] }"#,
        )
        .unwrap();
        assert!(Engine::load(dir.path(), None).is_err());
    }
}
