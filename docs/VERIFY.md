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

## M1 device acceptance — motorola edge 50 neo, Android 16, 2026-10-11

Debug APK, driven via adb uiautomator; numbers also cross-checked against the
on-device SQLite (`palmcode.db` pull) and the host mock server log.

| Check | Result |
|---|---|
| Cold start (force-stop → `am start -W`) | TotalTime **1038 ms** (< 1.2 s) |
| Provider probe vs Grid from the phone | models 34 / tokenize 403→heuristic / tool_choice OK / reasoning_content / final-chunk usage / cached_tokens exposed / **TTFT 410 ms, 51 tok/s**; panel + overrides shown |
| Keystore key survives app restart | force-stop + relaunch → straight to session list, no re-entry; `logcat -d` grep for key material: **0 hits** |
| Streaming turn vs Grid | reasoning collapsible + markdown answer + metrics `ttft 2142 ms · 7202.0 tok/s · 83 tok · cache 0%`; events+parts persisted (seq 1–8, coalesced) |
| http:// endpoint over `adb reverse` | cleartext banner in setup (before save) + persistent "http cleartext" chip in chat |
| Stop mid-stream | UI returned to Send; **mock server printed `client-disconnected observed`** |
| Jank @100 tok/s (bench mock, 20 s window) | **0.40% janky (13/3263, gfxinfo modern metric)** |
| Kill mid-stream (`am force-stop`) | relaunch → session statuses `interrupted`, partial text persisted; **Resume** row rendered at message end; tapping Resume regenerated into a NEW assistant message (msg 8 kept as partial) and stopped cleanly |
| Session list | 3 sessions, correct status/model/date rows; newest first |

Known cosmetic issue found and fixed during this run: root composables lacked
`safeDrawingPadding` (header collided with the status bar). Fixed in this
same commit. Driving notes (screen sleep, `&#183;` entities) are in
docs/NOTES.md.

## Verified on physical hardware

Verified via the probe above (identical app, post-rename `dev.palmcode`).

| Field | Result |
|---|---|
| Device | motorola edge 50 neo, Android 16 (API 36), arm64-v8a |
| Date | 2026-10-10 |
| FFI greeting | `palmcode core 0.1.0 online, hello android` |
| Exec-via-symlink | `exit=42 out="exec-probe ok pid=16563" (10 ms)` |
| userland busybox 1.38.0 arm64 executes on-device | physical (motorola edge 50 neo, Android 16, 2026-10-11) | banner OK; 367 applets; `sh -c 'read x...'` pipe OK; `awk` OK |
| userland ripgrep 15.2.0 arm64 executes on-device | physical (same device, 2026-10-11) | `rg --version` OK; literal match + `-c` count OK |
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
  acd53f1e…04d20a per gradle.org/release-checksums); dependency verification
  metadata: 341 components sha256-pinned (cross-OS aapt2 variants manually
  pinned + sha1-cross-checked).
- Release APK reproducibility: CI `repro` job (ubuntu-24.04). 2026-10-10:
  DETERMINISTIC same-dir. 2026-10-11 hardened (PR #5, run 38094399051):
  two checkouts in different dirs, different HOME + GRADLE_USER_HOME,
  RUSTFLAGS --remap-path-prefix active - **DETERMINISTIC, all three APKs
  byte-identical** (arm64-v8a de2fcbba..., universal f21cb8c7...,
  x86_64 d82ba334...).

## Assumed, not yet verified

- F-Droid build-server acceptance of the layout (`docs/fdroid/` draft; M5).
- Phantom-process limits on Android 12+ (docs/TROUBLESHOOTING.md), M3.
- External-storage performance delta; measurement M5.
- busybox / ripgrep cross-builds (M3; sizes in ADR 0008 are estimates).
