# NOTES — carried-forward housekeeping

Not code yet. Each item names the milestone that consumes it.

## M3 (userland wiring)

- **busybox applet symlinks must be named after applets**, with the binary
  itself shipped/aliased as exactly `busybox`. busybox dispatches on
  `argv[0]` basename; invoking the binary as e.g. `palmcode-busybox`
  fails with "applet not found". Ship as `libbusybox.so` (extract-to-disk
  packaging like the exec probe) and symlink `files/bin/busybox` ->
  `lib/.../libbusybox.so`, plus one symlink per applet
  (`files/bin/sh -> busybox`, ...). Verified on-device 2026-10-11
  (motorola edge 50 neo, Android 16): 367 applets run when named
  correctly, fail otherwise.
- **Nested exec regression pass before M3 ships**: `find | xargs`,
  subshells, `sed -i`, `tar` pipelines on a second physical device —
  ideally Samsung plus one Xiaomi/Oppo — and on one Android 14-or-older
  device or emulator image. Current on-device evidence is a single
  motorola device on Android 16 only.

## Release packaging (M3+; recorded now)

- **Release APK is arm64-v8a only.** x86_64 userland binaries must be
  excluded from release builds (they exist only for emulator dev/debug).
  Current state: release arm64 APK contains no userland binaries at all
  (verified by APK listing 2026-10-11; userland still lives outside the
  repo as spike artifacts) — the exclusion must be re-verified once M3
  wires userland into `jniLibs`. Release arm64 reference size:
  22.04 MB unsigned (splits config emits x86_64+universal for dev;
  restrict `assembleRelease` outputs to arm64 when userland lands — an
  x86_64 release with userland would needlessly carry rg/busybox x86_64).
