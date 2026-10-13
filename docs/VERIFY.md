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

## M2 pre-work: R8/minified release runtime - motorola edge 50 neo, 2026-10-12

Release APK (5.14 MB, R8 + shrinkResources) debug-signed and installed over
the debug build (user data + Keystore key survived). First signed build
failed ALL HTTPS: R8 had stripped `org.rustls.platformverifier.**` classes
that Rust looks up by exact JNI name; fixed with a keep rule - this is the
only R8 regression found by probing.

| Check | Result |
|---|---|
| Cold start x4 (`am start -W`) | **463 / 331 / 316 / 301 ms** |
| Probe vs Grid (R8 build) | models/tokenize-403->heuristic/tool_choice/reasoning_content/final-chunk usage/cached_tokens all OK; **TTFT 309 ms, 37.8 tok/s** |
| Streaming turn @100 tok/s via mock+reverse | streamed 60+ s, jank **0.00% (0/7310 frames)** |
| Stop mid-stream | mock: `client-disconnected observed` |
| APK arm64 release unsigned | **5.14 MB** (was 25.74 MB unshrunk) - composition in BENCHMARKS.md |
| CI (PR #6) incl. R8 byte-repro | green, both runs |

## M2 device acceptance — motorola edge 50 neo, Android 16, 2026-10-12

Debug APK (post-M2 core: tools, permissions, workspaces, prompt templates,
tree-sitter checks), driven via adb uiautomator, verified against on-disk
files (`run-as cat`).

| Check | Result |
|---|---|
| SAF workspace import | Download/demo-ws tree imported via system picker to `/data/user/0/dev.palmcode/files/workspaces/demo-ws` (runtime copy of app-dir files as app uid — `run-as cp` to FUSE is SELinux-blocked, so SAF is also the honest user flow) |
| Workspace survives process death | force-stop → relaunch → `workspace: demo-ws` chip present (registry restore at engine start) |
| Tools over the wire | first attempt HTTP 400 — litellm requires OpenAI-nested tool objects; fixed in builder (fa0e165). After fix: full loop vs Grid |
| Multi-file refactor with approvals (beat→pulse, 2 files) | reasoning + 4 tool cards (`read/grep/edit/edit` ✓, 0–7 ms each), approval sheet per edit (Allow once ×2), on-disk result exact: call site + definition renamed, lib.rs untouched |
| Tool card expand | preview + unified diff (`--- a/…`/`+++ b/…` header + colored lines) rendered from `file_preimages` |
| Kill mid-tool (`am force-stop` during pending approval) | session → `interrupted`; resume → killed edits appear as ✗ cards and are **never re-run**; model re-read both files, re-issued fresh calls, turn completed; final files: `tick_work()` once, `pulse()` intact, lib.rs updated |
| FUSE path safety | `/storage/emulated/0/…` roots: `realpath` blocked for app UIDs → RootSet falls back to lexical root (fbf20d8); adb-pushed-and-chowned paths stay unreadable by design |
| Organic user probe (owner, unscripted) | "hello / hoi / what all you can do" — model answered in BUILD mode with workspace-aware capability list |

Issues found live and fixed same-day: OpenAI tool shape (400), `s`
unused-binding on linux (CI rust), C-side path embeds breaking byte-repro
(CI repro), cache ratio >100% from crossing iterations (ae441ea, fix not yet
on-device), decode_tps <250 ms clamp (host-side, pending device).

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
  x86_64 d82ba334...). 2026-10-12 M2 regression hunt: tree-sitter C asserts
  (`__FILE__`) broke .so determinism across build dirs — fixed via
  `CFLAGS_*=-DNDEBUG -ffile-prefix-map=<core>=core` in the Gradle cargo env
  (669dc8f); local A/B .so hashes before DE470787…/8B5CB43E…, after
  identical 37B4AC1B…; CI green (run 38348300981).

## Assumed, not yet verified

- F-Droid build-server acceptance of the layout (`docs/fdroid/` draft; M5).
- Phantom-process limits on Android 12+ (docs/TROUBLESHOOTING.md), M3.
- External-storage performance delta; measurement M5.
- busybox / ripgrep cross-builds (M3; sizes in ADR 0008 are estimates).
