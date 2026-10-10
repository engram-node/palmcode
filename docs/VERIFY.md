# VERIFY

What has actually been run, and where. Claims anywhere else in this repo are
unverified unless listed here.

**Claim discipline:** an ABI/devices claim only counts as verified when it
ran on *physical* hardware, listed below with date. Emulator results are
called out as emulator-only.

## One-command device probe (canonical gate)

Requires: phone with USB debugging on, this machine's SDK + NDK, and a built
debug APK (`./gradlew assembleDebug`).

```powershell
powershell -ExecutionPolicy Bypass -File scripts/phone-probe.ps1 -Serial <adb-serial>
```

It prints a markdown table (paste it back, or file it below), covering:
FFI greeting rendered • exec-through-symlink (exit code, stdout, spawn ms) •
16 KB `p_align` check of the natives installed on the phone • on-device git
cycle (needs the stripped aarch64 git from `userland/spike/`, else SKIP).

Paste-area template — one row per device, newest first:

| Field | Result |
|---|---|
| Device | <model>, Android <ver> (API <level>), <abi> |
| Date | <yyyy-mm-dd> |
| FFI greeting | `<greeting>` |
| Exec-via-symlink | `<exit=42 … ms>` |
| Symlink target | `<files/bin/probe -> …/lib<abi>/libexecprobe.so>` |
| 16 KB alignment | <lib*.so: ok; …> |
| git on-device | <version; cycle result; stripped size> |

## Verified on physical hardware

Verified via the probe above (identical app, post-rename `dev.palmcode`).

| Field | Result |
|---|---|
| Device | motorola edge 50 neo, Android 16 (API 36), arm64-v8a |
| Date | 2026-10-10 |
| FFI greeting | `palmcode core 0.1.0 online, hello android` |
| Exec-via-symlink | `exit=42 out="exec-probe ok pid=16563" (10 ms)` |
| Symlink target | `/data/user/0/dev.palmcode/files/bin/probe -> …/lib/arm64/libexecprobe.so` |
| 16 KB alignment | libandroidx.graphics.path.so: ok; libexecprobe.so: ok; libjnidispatch.so: ok; libpalmcode_ffi.so: ok |
| git on-device | git version 2.56.0; init/add/commit x2/log/status OK; 3.55 MB |

Earlier M0-era run (pre-rename `dev.pocket`, same mechanics): exec ~11 ms,
git cycle OK on the same phone (commit `e26be30`, PR #3).

## Verified on device (emulator only — x86_64)

**Emulator `palmcode34` (x86_64, Android 14 / API 34, google_apis, WHPX)**,
2026-10-10: APK installs; greeting renders; exec-through-symlink exit 42,
~8 ms spawn. Emulator-only: does not substitute for the physical section.

## Verified on host + CI

- `cargo build/test/clippy --workspace` + `fmt --check` + `cargo deny`:
  green (Windows host; ubuntu-24.04 CI).
- Vendored offline build (`core/vendor`, byte-exact via
  `.gitattributes core/vendor/** -text`): green on Windows AND linux CI.
- arm64 `libpalmcode_ffi.so` LOAD segments `p_align = 0x4000` — host-side
  artifact check ✓ (device row above is authoritative for runtime).
- GitHub Actions: rust + android jobs green on PR #1; repro job added in the
  M1-prep PR (this branch), first run reported there.
- Gradle wrapper pinned (`distributionSha256Sum` =
  acd53f1e…04d20a per gradle.org/release-checksums).

## Assumed, not yet verified

- Bit-for-bit reproducible release APK (new `repro` CI job: first verdict
  pending on this branch).
- F-Droid build-server acceptance of the layout (`docs/fdroid/` draft; M5).
- Phantom-process limits on Android 12+ (docs/TROUBLESHOOTING.md), M3.
- External-storage performance delta; measurement M5.
- busybox / ripgrep cross-builds (M3; sizes in ADR 0008 are estimates).
