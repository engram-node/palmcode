# VERIFY

What has actually been run, and where. Claims anywhere else in this repo are
unverified unless listed here.

## Verified on device

**Android Emulator `pocket34` (x86_64, Android 14 / API 34, google_apis, WHPX)**

2026-10-10:

- APK installs and launches; UI renders the Rust greeting
  "pocket core 0.1.0 online, hello android" (UniFFI → JNA → `libpocketffi.so`). ✓
- **Spike B (exec via symlink): PASS.** App created
  `/data/user/0/dev.pocket/files/bin/probe` → `.../lib/x86_64/libexecprobe.so`
  and `Runtime.exec()`'d it through the symlink: stdout captured
  ("exec-probe ok pid=7384"), exit code 42, ~8 ms spawn.
  `useLegacyPackaging=true`, targetSdk 36. (repeat on physical hardware TBD)
- `platforms;android-36` + `build-tools;36.0.0` + NDK 30.0.16248370 +
  `pocket34` AVD: `./gradlew assembleDebug` green end to end;
  `assembleRelease` green, unsigned APK 23.5 MB. ✓

## Verified on host (Windows 11, this machine) and CI-relevant

- `cargo build/test/clippy --workspace` + `cargo fmt --check`: green.
- `cargo deny check` (advisories, bans, licenses, sources): green
  (binary: prebuilt 0.20.2 MSVC build, sha256-verified; see ADR 0001 —
  0.20.2 does not compile for `x86_64-pc-windows-gnu`).
- Vendored offline build: `cargo build --offline --workspace` green with an
  empty cargo registry cache.
- arm64 `libpocketffi.so` (NDK r30 default link flags): all LOAD segments
  `p_align = 0x4000` (16 KB page-size alignment requirement). ✓
- **Spike A (git cross-build): FEASIBLE, verified end to end (ADR 0006).**
  `git` 2.56.0 built from sha256-verified source for aarch64 AND x86_64
  (upstream configure + two ac_cv overrides + three `config.mak` knobs);
  the x86_64 binary ran on the emulator: `git version 2.56.0`,
  init/add/commit (40d8c84)/log/status/diff all OK. arm64 binary LOAD
  segments all `p_align=0x4000` (16 KB). ~3.9 MB stripped per ABI.
  Open: HTTPS remotes (curl+TLS) — deferred to M3, see ADR 0006.

## Assumed, not yet verified

- Exec-via-symlink on a **physical** device (spec asks for real hardware; the
  emulator result above is the stand-in until then).
- `assembleRelease` from a pristine clone (clean-clone build: planned next).
- GitHub Actions CI: rust job (fmt/clippy/test/deny) + android job
  (NDK r30 cross-build both ABIs, `assembleDebug`/`assembleRelease`,
  APK artifact) — green on PR #1 (ubuntu-24.04). ✓
- F-Droid build-server acceptance of the layout (`docs/fdroid/` draft; M5).
- Phantom-process limits on Android 12+ (docs/TROUBLESHOOTING.md), M3.
- External-storage (`/storage/emulated/0`) performance delta; measurement M5.
- cold `cargo ndk` x86_64 abi (used by the emulator APK — implicitly proven by
  the pass above).
