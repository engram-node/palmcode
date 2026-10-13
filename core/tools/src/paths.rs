//! Path safety: every tool call resolves against the session's granted
//! roots before touching the filesystem.
//!
//! Threat model: the model (or a prompt-injection inside a file it read)
//! may attempt to escape the workspace via `..`, absolute paths, symlinks,
//! NTFS alternate data streams, or case tricks. FS-level traversal needs a
//! literal `..` component at the kernel, so lexical normalization plus
//! canonicalization of the existing prefix closes real escapes; Unicode
//! lookalikes cannot traverse at FS level and are left untouched (they are
//! policy-matching concerns, handled in `perms`).

use std::path::{Component, Path, PathBuf};

use crate::error::{Result, ToolError};

/// Workspace roots granted to a session, canonicalized once.
pub struct RootSet {
    roots: Vec<PathBuf>,
}

impl RootSet {
    pub fn new(roots: impl IntoIterator<Item = PathBuf>) -> Result<Self> {
        let mut out = Vec::new();
        for r in roots {
            // shortcut: Android FUSE (/storage/emulated, /sdcard) blocks
            // realpath() traversal for app processes — keep the lexical root
            // and containment stays lexical too; upgrade: canonicalize via a
            // SAF document-id instead of realpath.
            let root = std::fs::canonicalize(&r)
                .map(|c| strip_verbatim(&c))
                .unwrap_or_else(|_| {
                    tracing::warn!(
                        root = %r.display(),
                        "canonicalize failed; using lexical workspace root"
                    );
                    lexical_normalize(&r)
                });
            if !root.is_dir() {
                return Err(ToolError::BadPath(format!(
                    "workspace root is not a directory: {}",
                    root.display()
                )));
            }
            out.push(root);
        }
        Ok(Self { roots: out })
    }

    pub fn roots(&self) -> &[PathBuf] {
        &self.roots
    }

    /// Resolve `user` (absolute, or relative to `base` / the first root)
    /// into a path guaranteed to stay inside a granted root. The target
    /// does not need to exist (write tool); its deepest existing ancestor
    /// is canonicalized so symlink escapes are still caught.
    pub fn resolve(&self, user: &str, base: Option<&Path>) -> Result<PathBuf> {
        if user.is_empty() {
            return Err(ToolError::BadPath("empty path".into()));
        }
        if user.chars().any(|c| c.is_control()) {
            return Err(ToolError::BadPath("control character in path".into()));
        }
        // NTFS alternate data streams / device names: reject ':' anywhere
        // except a leading drive letter (Windows) — on Unix ':' is a legal
        // char but rare and confusing; rejecting is the smaller risk.
        #[cfg(windows)]
        {
            let bytes = user.as_bytes();
            for (i, &b) in bytes.iter().enumerate() {
                if b == b':' && !(i == 1 && bytes[0].is_ascii_alphabetic()) {
                    return Err(ToolError::BadPath(format!("':' not allowed in: {user}")));
                }
            }
        }

        let raw = Path::new(user);
        let joined = if raw.is_absolute() {
            raw.to_path_buf()
        } else {
            match base {
                Some(b) => b.join(raw),
                None => self
                    .roots
                    .first()
                    .ok_or_else(|| ToolError::BadPath("no workspace roots".into()))?
                    .join(raw),
            }
        };

        // 1. Lexically normalize: fold `.` and process `..` WITHOUT touching
        //    the FS — a `..` above the (not-yet-checked) path is fine at
        //    this stage; the containment check happens after resolution.
        let norm = lexical_normalize(&joined);

        // 2. Canonicalize the deepest existing ancestor to resolve symlinks,
        //    then re-append the non-existent tail. When canonicalize is
        //    blocked (Android FUSE roots, see RootSet::new) fall back to the
        //    lexical form — the same basis the roots use, so containment
        //    stays consistent.
        let resolved = if norm.exists() {
            std::fs::canonicalize(&norm)
                .map(|c| strip_verbatim(&c))
                .unwrap_or_else(|_| norm.clone())
        } else {
            let mut ancestor = norm.clone();
            let mut tail: Vec<std::ffi::OsString> = Vec::new();
            loop {
                if ancestor.exists() {
                    break;
                }
                match ancestor.file_name() {
                    Some(name) => {
                        tail.push(name.to_os_string());
                        ancestor.pop();
                    }
                    None => return Err(ToolError::BadPath(format!("no such path: {user}"))),
                }
            }
            let mut base = std::fs::canonicalize(&ancestor)
                .map(|c| strip_verbatim(&c))
                .unwrap_or_else(|_| ancestor.clone());
            for comp in tail.iter().rev() {
                base.push(comp);
            }
            base
        };

        // 3. Containment: resolved must equal or live under a root.
        if self.roots.iter().any(|root| starts_with(&resolved, root)) {
            Ok(resolved)
        } else {
            Err(ToolError::Escape(user.to_string()))
        }
    }
}

/// Case-insensitive on Windows, exact elsewhere.
fn starts_with(path: &Path, root: &Path) -> bool {
    #[cfg(windows)]
    {
        let p = path.as_os_str().to_string_lossy().to_lowercase();
        let r = root.as_os_str().to_string_lossy().to_lowercase();
        let p = p.trim_end_matches('\\');
        let r = r.trim_end_matches('\\');
        p == r || p.starts_with(&format!("{r}\\"))
    }
    #[cfg(not(windows))]
    {
        path == root || path.starts_with(root)
    }
}

/// Strip the Windows `\\?\` verbatim prefix that `canonicalize` adds, so
/// comparisons and UI see ordinary paths.
pub fn strip_verbatim(p: &Path) -> PathBuf {
    #[cfg(windows)]
    if let Some(stripped) = p
        .as_os_str()
        .to_str()
        .and_then(|s| s.strip_prefix(r"\\?\"))
    {
        return PathBuf::from(stripped);
    }
    p.to_path_buf()
}

fn lexical_normalize(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                // Pop only if we have a normal component to pop; popping the
                // root/prefix is what the containment check rejects later.
                if !out.pop() {
                    out.push("..");
                }
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn setup() -> (tempfile::TempDir, RootSet) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("repo");
        fs::create_dir_all(root.join("sub/deep")).unwrap();
        fs::write(root.join("sub/deep/f.rs"), "fn main() {}").unwrap();
        fs::write(dir.path().join("outside.txt"), "secret").unwrap();
        let rs = RootSet::new(vec![root]).unwrap();
        (dir, rs)
    }

    #[test]
    fn resolves_relative_against_first_root() {
        let (_d, rs) = setup();
        let p = rs.resolve("sub/deep/f.rs", None).unwrap();
        assert!(p.ends_with(Path::new("sub").join("deep").join("f.rs")));
    }

    #[test]
    fn root_itself_and_dot_ok() {
        let (_d, rs) = setup();
        assert_eq!(rs.resolve(".", None).unwrap(), rs.roots()[0]);
        assert_eq!(rs.resolve("./", None).unwrap(), rs.roots()[0]);
    }

    #[test]
    fn parent_escape_denied() {
        let (_d, rs) = setup();
        assert!(matches!(
            rs.resolve("../outside.txt", None),
            Err(ToolError::Escape(_))
        ));
        assert!(matches!(
            rs.resolve("sub/../../outside.txt", None),
            Err(ToolError::Escape(_))
        ));
        assert!(matches!(
            rs.resolve("../../../../../../etc/passwd", None),
            Err(ToolError::Escape(_)) | Err(ToolError::BadPath(_))
        ));
    }

    #[test]
    fn absolute_outside_denied() {
        let (d, rs) = setup();
        let outside = d.path().join("outside.txt");
        assert!(matches!(
            rs.resolve(outside.to_str().unwrap(), None),
            Err(ToolError::Escape(_))
        ));
    }

    #[test]
    fn absolute_inside_allowed() {
        let (_d, rs) = setup();
        let inside = rs.roots()[0].join("sub/deep/f.rs");
        assert_eq!(rs.resolve(inside.to_str().unwrap(), None).unwrap(), inside);
    }

    #[test]
    fn dotdot_that_stays_inside_is_fine() {
        let (_d, rs) = setup();
        assert_eq!(
            rs.resolve("sub/../sub/deep/f.rs", None).unwrap(),
            rs.roots()[0].join("sub/deep/f.rs")
        );
    }

    #[test]
    fn nonexistent_new_file_inside_ok() {
        let (_d, rs) = setup();
        let p = rs.resolve("sub/new/branch/file.txt", None).unwrap();
        assert!(p.ends_with(Path::new("sub").join("new").join("branch").join("file.txt")));
    }

    #[test]
    fn symlink_escape_denied() {
        let (_d, rs) = setup();
        let root = rs.roots()[0].clone();
        let link = root.join("linkout");
        let target = root.parent().unwrap().to_path_buf();
        #[cfg(unix)]
        let made = std::os::unix::fs::symlink(&target, &link).is_ok();
        #[cfg(windows)]
        let made = std::os::windows::fs::symlink_dir(&target, &link).is_ok();
        if !made {
            eprintln!("symlink creation unavailable; skipping");
            return;
        }
        assert!(matches!(
            rs.resolve("linkout/outside.txt", None),
            Err(ToolError::Escape(_))
        ));
    }

    #[test]
    fn unicode_lookalike_dots_stay_inside_not_denied() {
        // Fullwidth full stop is a literal directory name at FS level, NOT
        // a traversal — must resolve inside the root without false deny.
        let (_d, rs) = setup();
        fs::create_dir_all(rs.roots()[0].join("\u{FF0E}\u{FF0E}/data")).unwrap();
        let p = rs.resolve("\u{FF0E}\u{FF0E}/data", None).unwrap();
        assert!(p.starts_with(&rs.roots()[0]));
    }

    #[cfg(windows)]
    #[test]
    fn case_variant_of_root_allowed() {
        let (_d, rs) = setup();
        let upper = rs.roots()[0]
            .to_string_lossy()
            .to_uppercase()
            .replace(":\\\\", ":\\");
        let cand = PathBuf::from(&upper).join("sub/deep/f.rs");
        // canonicalize returns the real casing; containment must accept.
        let r = rs.resolve(cand.to_str().unwrap(), None);
        assert!(r.is_ok(), "case variant denied: {r:?}");
    }

    #[cfg(windows)]
    #[test]
    fn ntfs_ads_rejected() {
        let (_d, rs) = setup();
        assert!(matches!(
            rs.resolve("sub/deep/f.rs:hidden", None),
            Err(ToolError::BadPath(_))
        ));
    }

    #[test]
    fn empty_and_control_rejected() {
        let (_d, rs) = setup();
        assert!(matches!(rs.resolve("", None), Err(ToolError::BadPath(_))));
        assert!(matches!(
            rs.resolve("a\u{0007}b", None),
            Err(ToolError::BadPath(_))
        ));
    }
}
