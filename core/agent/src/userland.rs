//! Fail-closed userland preparation with injectable filesystem operations.
use std::path::{Path, PathBuf};

pub(crate) trait FarmOps {
    fn mkdir(&mut self, path: &Path) -> std::io::Result<()>;
    fn link(&mut self, target: &Path, link: &Path) -> std::io::Result<()>;
    fn executable(&mut self, path: &Path) -> bool;
}

pub(crate) fn prepare(
    lib_dir: Option<PathBuf>,
    farm: &Path,
    ops: &mut impl FarmOps,
) -> Result<PathBuf, String> {
    let check = || -> Result<PathBuf, String> {
        let lib_dir = lib_dir.ok_or("native library lookup failed")?;
        ops.mkdir(farm)
            .map_err(|_| "cannot create applet directory")?;
        for (binary, names) in [
            ("libbusybox.so", palmcode_exec::BUSYBOX_APPLETS),
            ("librg.so", &["rg"][..]),
            ("libgit.so", &["git"][..]),
        ] {
            let target = lib_dir.join(binary);
            if !ops.executable(&target) {
                return Err(format!("non-executable packaged entry: {binary}"));
            }
            for name in names {
                let link = farm.join(name);
                ops.link(&target, &link)
                    .map_err(|_| format!("cannot refresh applet: {name}"))?;
                if !ops.executable(&link) {
                    return Err(format!("non-executable applet: {name}"));
                }
            }
        }
        Ok(farm.join("sh"))
    };
    check().map_err(|reason| format!("userland unavailable: {reason}"))
}

#[cfg(unix)]
pub(crate) struct NativeOps;

#[cfg(unix)]
impl FarmOps for NativeOps {
    fn mkdir(&mut self, path: &Path) -> std::io::Result<()> {
        std::fs::create_dir_all(path)
    }
    fn link(&mut self, target: &Path, link: &Path) -> std::io::Result<()> {
        if std::fs::read_link(link).is_ok_and(|current| current == target) {
            return Ok(());
        }
        match std::fs::symlink_metadata(link) {
            Ok(_) => std::fs::remove_file(link)?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
        std::os::unix::fs::symlink(target, link)
    }
    fn executable(&mut self, path: &Path) -> bool {
        use std::os::unix::ffi::OsStrExt;
        let Ok(path_c) = std::ffi::CString::new(path.as_os_str().as_bytes()) else {
            return false;
        };
        path.is_file() && unsafe { libc::access(path_c.as_ptr(), libc::X_OK) == 0 }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[derive(Default)]
    struct Injected {
        fail: &'static str,
        links: usize,
    }
    impl FarmOps for Injected {
        fn mkdir(&mut self, _: &Path) -> std::io::Result<()> {
            if self.fail == "mkdir" {
                Err(std::io::Error::other("injected"))
            } else {
                Ok(())
            }
        }
        fn link(&mut self, _: &Path, _: &Path) -> std::io::Result<()> {
            self.links += 1;
            if self.fail == "link" {
                Err(std::io::Error::other("injected"))
            } else {
                Ok(())
            }
        }
        fn executable(&mut self, path: &Path) -> bool {
            self.fail != "target" && !(self.fail == "entry" && path.starts_with("farm"))
        }
    }
    #[test]
    fn lookup_and_all_filesystem_failures_are_closed() {
        assert!(
            prepare(None, Path::new("farm"), &mut Injected::default())
                .unwrap_err()
                .starts_with("userland unavailable")
        );
        for fail in ["mkdir", "link", "target", "entry"] {
            let mut ops = Injected {
                fail,
                ..Default::default()
            };
            assert!(
                prepare(Some("lib".into()), Path::new("farm"), &mut ops)
                    .unwrap_err()
                    .starts_with("userland unavailable"),
                "{fail}"
            );
        }
    }
    #[test]
    fn every_start_revalidates_every_entry() {
        let mut ops = Injected::default();
        for run in 1..=2 {
            assert_eq!(
                prepare(Some("lib".into()), Path::new("farm"), &mut ops).unwrap(),
                Path::new("farm/sh")
            );
            assert_eq!(ops.links, run * (palmcode_exec::BUSYBOX_APPLETS.len() + 2));
        }
        ops.fail = "entry";
        assert!(prepare(Some("lib".into()), Path::new("farm"), &mut ops).is_err());
    }
    #[cfg(unix)]
    #[test]
    fn stale_links_replaced_obstructions_and_nonexecutables_rejected() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let lib = dir.path().join("lib");
        let farm = dir.path().join("farm");
        std::fs::create_dir(&lib).unwrap();
        for name in ["libbusybox.so", "librg.so", "libgit.so"] {
            std::fs::write(lib.join(name), "fixture").unwrap();
            std::fs::set_permissions(lib.join(name), std::fs::Permissions::from_mode(0o700))
                .unwrap();
        }
        std::fs::create_dir(&farm).unwrap();
        std::os::unix::fs::symlink("old-packaged-path", farm.join("sh")).unwrap();
        prepare(Some(lib.clone()), &farm, &mut NativeOps).unwrap();
        assert_eq!(
            std::fs::read_link(farm.join("sh")).unwrap(),
            lib.join("libbusybox.so")
        );
        std::fs::remove_file(farm.join("rg")).unwrap();
        std::fs::create_dir(farm.join("rg")).unwrap();
        assert!(prepare(Some(lib.clone()), &farm, &mut NativeOps).is_err());
        std::fs::remove_dir(farm.join("rg")).unwrap();
        std::fs::set_permissions(
            lib.join("libgit.so"),
            std::fs::Permissions::from_mode(0o600),
        )
        .unwrap();
        assert!(prepare(Some(lib), &farm, &mut NativeOps).is_err());
    }
}
