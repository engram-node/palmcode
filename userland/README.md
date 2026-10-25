# userland

Reproducible, checksum-verified source builds of the binaries palmcode ships as
`lib<name>.so` in `jniLibs`:

- **busybox** (ash + applet set)
- **ripgrep**
- **git** (real upstream git, cross-compiled with the NDK; spike first, see
  `docs/decisions/0004-userland-version-pins.md`)
- **exec-probe** (tiny C program used by the M0 spike; not shipped in release
  builds of the userland from M3 on)

`build.sh` (M3) is invoked by Gradle, downloads pinned tarballs
(`versions.env`), verifies sha256, builds with the NDK clang, and outputs to
`android/app/build/userland/jniLibs/<abi>/`. Nothing built here is committed.

BusyBox starts from `allnoconfig`; `busybox-applets.txt` selects 125 applets
and their upstream Kconfig symbols. Administrative boot, mount, module,
login and service applets are excluded at compilation. The build compares
the compiled dispatch table with the allowlist and the Rust prompt catalog
for each ABI and rejects drift or an applet count outside 100–150.
After editing the allowlist, regenerate the catalog:

```
bash userland/generate-catalog.sh > core/exec/src/applets.rs
```

Requires a POSIX environment with make + a C toolchain (MSYS2 on Windows,
any Linux on F-Droid).
