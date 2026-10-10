# 0007: Binaries shipped as `lib*.so`, executed via symlinks (spike B result)

Date: 2026-10-10. Status: verified (emulator + physical Motorola Edge 50 Neo,
Android 16: exit 42, stdout correct, ~11 ms spawn).

## Context

Apps targeting API 29+ cannot `exec()` files from their own data directories
(W^X). The only executable filesystem an app owns is its extracted native
library directory. The userland plan (busybox, git, rg) depends on this path.

## Result (see docs/VERIFY.md)

- `libexecprobe.so` compiled by NDK clang, packaged under jniLibs,
  `packaging.jniLibs.useLegacyPackaging=true` → extracted at install.
- App symlinked `files/bin/probe` → `<nativeLibraryDir>/libexecprobe.so` and
  `Runtime.exec()`'d **the symlink**: correct stdout, exit code, ~8 ms spawn.
- Android 14 x86_64 emulator, targetSdk 36. Physical-device confirmation is
  still open (VERIFY.md).

## Consequences for design

- The symlink farm approach is confirmed: M3's `bash` tool gets
  `files/bin/` on `PATH` with applet symlinks into `nativeLibraryDir`.
- **Strict naming rule:** every shipped binary must be `lib<name>.so` and
  produced from source in `userland/build.sh`; AGP only packages files that
  match `lib*.so`.
- The probe (`userland/exec-probe/hello.c`, Gradle task `execProbe*`) stays as
  the permanent canary: if exec regresses after AGP/platform bumps, M3's
  permissions/executor work catches it immediately rather than at runtime.

## Alternatives rejected

- Legacy targetSdk (<29) to keep exec-from-data: rejected in the brief for
  targetSdk currency; kept as possible post-v1 flavor.
- Loading binaries via `android:extractNativeLibs=false` + in-memory exec:
  impossible — exec needs a filesystem path.
