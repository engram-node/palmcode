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
| Provider probe vs gateway from the phone | models 34 / tokenize 403→heuristic / tool_choice OK / reasoning_content / final-chunk usage / cached_tokens exposed / **TTFT 410 ms, 51 tok/s**; panel + overrides shown |
| Keystore key survives app restart | force-stop + relaunch → straight to session list, no re-entry; `logcat -d` grep for key material: **0 hits** |
| Streaming turn vs gateway | reasoning collapsible + markdown answer + metrics `ttft 2142 ms · 7202.0 tok/s · 83 tok · cache 0%`; events+parts persisted (seq 1–8, coalesced) |
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
| Probe vs gateway (R8 build) | models/tokenize-403->heuristic/tool_choice/reasoning_content/final-chunk usage/cached_tokens all OK; **TTFT 309 ms, 37.8 tok/s** |
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
| Tools over the wire | first attempt HTTP 400 — litellm requires OpenAI-nested tool objects; fixed in builder (fa0e165). After fix: full loop vs gateway |
| Multi-file refactor with approvals (beat→pulse, 2 files) | reasoning + 4 tool cards (`read/grep/edit/edit` ✓, 0–7 ms each), approval sheet per edit (Allow once ×2), on-disk result exact: call site + definition renamed, lib.rs untouched |
| Tool card expand | preview + unified diff (`--- a/…`/`+++ b/…` header + colored lines) rendered from `file_preimages` |
| Kill mid-tool (`am force-stop` during pending approval) | session → `interrupted`; resume → killed edits appear as ✗ cards and are **never re-run**; model re-read both files, re-issued fresh calls, turn completed; final files: `tick_work()` once, `pulse()` intact, lib.rs updated |
| FUSE path safety | `/storage/emulated/0/…` roots: `realpath` blocked for app UIDs → RootSet falls back to lexical root (fbf20d8); adb-pushed-and-chowned paths stay unreadable by design |
| Organic user probe (owner, unscripted) | "hello / hoi / what all you can do" — model answered in BUILD mode with workspace-aware capability list |
| Release (R8) smoke, same device 2026-10-13 | read-only turn: `✓ ls src · 8 ms` card, correct answer; metrics `ttft 869 ms · 34.4 tok/s · 105 tok · cache 55%` (cache-ratio fix confirmed live: ≤100%) |
| Release cold start ×4 (`am start -W`) | **537 / 389 / 346 / 345 ms** (< 1.2 s) — tree-sitter .so costs ~40–70 ms vs M1 |
| Release arm64 unsigned APK | **7.24 MB** (M1: 5.14 MB; target 30 MB / hard 40 MB — green) |

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

## M3-A nested-exec spike — emulator `pocket34`, PROVISIONAL (phone rerun pending)

**CONFIRMED** per repo policy (device evidence required): identical results
on the emulator and on the phone. Emulator `pocket34` (x86_64, Android 14 /
API 34, sdk_gphone64_x86_64), 2026-10-22. Phone motorola edge 50 neo
(arm64-v8a, Android 16, `ZD222NNHWP`), 2026-10-23: **14/14 PASS identical to
the table below**. Reproduce:
`powershell -File scripts/exec-matrix.ps1 -Serial emulator-5554 -Abi x86_64`
(phone: `-Serial ZD222NNHWP -Abi arm64-v8a`).

| case | expected | got | result |
|---|---|---|---|
| argv[0] applet dispatch (sh via symlink) | dispatch-ok | dispatch-ok | PASS |
| busybox --list applet count | > 300 | 367 | PASS |
| exit code passthrough | 7 | 7 | PASS |
| signal death exit code (TERM) | 143 | 143 | PASS |
| 3-deep nested sh -c | deep-ok | deep-ok | PASS |
| shebang #!/bin/sh | shebang-bin-sh | shebang-bin-sh | PASS |
| shebang #!/usr/bin/env sh (raw exec) | FAIL expected (no /usr/bin/env) | `./t2.sh: No such file or directory` | PASS |
| shebang env mitigation (invoke via sh) | shebang-env-sh | shebang-env-sh | PASS |
| redirect + applet chain (`wc -l < f`) | 2 | 2 | PASS |
| stdin closed -> immediate EOF | 0, exit 0 | 0 | PASS |
| rg --version | ripgrep | ripgrep 15.2.0 (PCRE2 off) | PASS |
| git --version | git version | git version 2.56.0 | PASS |
| pgid group kill (`kill -TERM -PGID`) reaps grandchildren | s2 empty | kill-exit 0, s2 empty (leader + 2 sleeps dead) | PASS |
| leader-only kill orphans grandchildren | orphans survive | 2 sleeps orphaned in dead pgid | PASS |

Findings and follow-up decisions: docs/decisions/0014 (`#!/usr/bin/env`
unfixable -> `sh script` convention in the command catalog; group-kill via
negative pgid — busybox kill has no `--` separator; orphan sweep on resume).
In-shell background jobs die with the spawning adb session (mksh job
control); in-app spawns are unaffected — consistent with the PDEATHSIG
design, to be verified live with deliverable E.

First phone run flagged a spurious group-kill FAIL: the fixture ps filter
(`sleep|grandchild`) also matched a Motorola vendor daemon
(`/vendor/bin/sleep 600`, pgid 2105) - a measurement artifact, not a kill
failure. Filter tightened to `sleep 45|grandchild`; rerun 14/14 (group kill
s2 = 0 survivors, leader-only s2 = exactly 2 orphans).

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

## M3-D/F/H bash tool + classifier + catalog - host-verified 2026-10-23, device runs pending

| Check | Result |
|---|---|
| bash tool (D) host tests | 5 unit (echo+exit line, exit 7 + spill, 1 s timeout kills tree, live sink, guard errors) + 1 end-to-end tool_loop (classifier -> executor -> batched ToolOutput -> loop back): **PASS** |
| Command classifier (F) | 12 suites, 70+ adversarial cases (IFS tricks, nested substitutions, heredoc expansion, eval, wrapper bypass): **PASS** |
| Bash approval caches (F) | exact-match session/project (glob metachars never broaden): **PASS** |
| Command catalog (H) | 367 busybox applets + rg/git + `sh script` note in the byte-stable prompt prefix (before env block) |
| Workspace suite | **27 suites / 161 tests green**, `cargo fmt --all --check` + `cargo clippy --workspace --all-targets -- -D warnings` clean (MSYS2 sh on host) |
| Android native farm + NativeProcExecutor (dladdr discovery, 367 applet symlinks, PATH=farm) | **PENDING** - deliverable E device run |
| Device kill-mid-command orphan check + screen-off 10 min bash run (FGS) | **PENDING** - phone/emulator acceptance |

## CI repair - 2026-10-24

- PR #7 run 39911955809: Rust fmt, Clippy and tests passed; cargo-deny
  failed on rustls and rustls-webpki advisories. Both release APK builds
  succeeded; comparison differed only in BusyBox for both ABIs.
- Patch releases: rustls 0.23.46, rustls-webpki 0.103.16, Tokio 1.53.3
  (the current audit also flags Tokio's AsyncFd I/O safety advisory).
  Cargo.lock checksums and vendored sources refreshed; offline source
  replacement restored after an interrupted vendor update.
- Windows host: `cargo fmt --all --check`,
  `cargo clippy --locked --offline --workspace --all-targets -- -D warnings`,
  and `cargo deny check`: PASS. All advisory, ban, license and source gates
  remain enabled.
- `cargo test --locked --offline --workspace`: PASS, 161 tests on Windows
  with MSYS2 sh on PATH.
- BusyBox: `KCONFIG_NOTIMESTAMP=1` produces an empty AUTOCONF_TIMESTAMP
  in generated autoconf.h. Two independent builds in different directories
  produce byte-identical stripped libbusybox.so files for both ABIs;
  the build's 16 KB LOAD alignment gate passes. SHA-256:
  arm64-v8a 1210eb96176639fda85a0b82dce5609b0f0f223b70843d29f644d0c04cc360d2;
  x86_64 94a3731f7823c3e18a5462ebc66f482adc329c008aa7d116685f73b297a53ca4.
  Full APK byte comparison awaits the next CI run.

## CI repair device verification - motorola edge 50 neo, 2026-10-24

Physical phone, Android 16 (API 36), arm64-v8a. Built debug and minified
release APKs from the repaired tree. Release was signed locally with the
debug certificate for installation; distribution release APKs remain unsigned.

| Check | Result |
|---|---|
| `assembleDebug assembleRelease` | PASS, both ABIs and universal APKs |
| Final APK native LOAD alignment | PASS, all 14 libraries across arm64-v8a and x86_64; artifact inspection, x86_64 not run on this phone |
| Packaged BusyBox reproducibility | Both ABI binaries match the earlier independent A/B builds exactly |
| Release APK userland on phone (`scripts/exec-matrix.ps1`) | 14/14 PASS: dispatch, 367 applets, exits/signals, nested shells, shebang handling, redirects/EOF, rg/git versions, process-group cleanup |
| Debug in-app Bash via loopback mock | Allow once -> successful Bash card -> final reply; packaged native farm used |
| Debug-to-release app update | Initially FAIL: stale farm symlinks retained the previous APK native directory. Repaired target refresh; repeated update and release Bash turn PASS |
| Unix symlink regression test on phone | `userland_link_tests::refreshes_dangling_links_after_apk_update`: 1/1 PASS, Android test executable cross-compiled from source and run via adb |
| Minified release cold start x4 | 359 / 368 / 387 / 363 ms; saved mock configuration returns to session list after force-stop |
| Host checks after device fixes | fmt, Clippy, 161 workspace tests PASS on Windows; Unix regression additionally passed on phone |
| Release unsigned APK size | arm64-v8a 12,015,044 bytes; x86_64 12,480,175 bytes; universal 22,009,462 bytes |

The scripted bench listener was also fixed to use nonblocking mode before
conversion to Tokio; its previous blocking mode stalled the initial test.
The original encrypted provider preferences were backed up only on-device,
restored byte-for-byte, and the temporary backup removed. The patched debug
APK is left installed. The local mock server and adb reverse mapping were
removed after testing. No live-provider HTTPS inference was exercised in
this run. Full APK reproducibility across independent checkouts remains a
CI check; local A/B verification covered BusyBox binaries.

## Assumed, not yet verified

- F-Droid build-server acceptance of the layout (`docs/fdroid/` draft; M5).
- Phantom-process limits on Android 12+ (docs/TROUBLESHOOTING.md), M3.
- External-storage performance delta; measurement M5.
- busybox / ripgrep cross-builds (M3; sizes in ADR 0008 are estimates).

| Device check | Status | Exact steps |
| --- | --- | --- |
| Screen off during bash | PENDING | On device, approve and run `sleep 900; printf 'screen-off-complete\n'`. Turn the screen off immediately, leave it off for 10 minutes, then unlock. Wait for completion at 15 minutes. Verify the completion marker appears once, output remains responsive, and the run finishes successfully. |
| Kill-mid-command orphan sweep | PENDING | On device, approve and run `sleep 300 & wait`. Before killing the app, use `adb shell ps -A -o PID,PPID,ARGS` to record the shell and sleep PIDs. Kill the app process with `adb shell run-as <application-id> kill -9 <app-pid>`, relaunch it, and run the same ps command. Verify both recorded child PIDs are absent after startup sweep. Repeat with `setsid sh -c 'sleep 300' & wait`. |
| Userland farm failure | PENDING | On a debuggable device build, use `adb shell run-as <application-id>` to remove one entry from the app-private `userland/bin` farm and replace its path with a directory. Start a bash call. Verify a clear `userland unavailable` error and no command execution. Remove the obstructing directory, restart, and verify the farm is recreated using the current packaged libraries. Repeat after an app upgrade with a stale symlink and verify it is replaced. |

| Date | Device verification | Result | Steps and limits |
| --- | --- | --- | --- |
| 2026-10-25 | Material You debug install and launch, Android API 36 | PASS | Built `:android:app:assembleDebug`, installed `app-universal-debug.apk` with `adb install -r`, and cold-launched `dev.palmcode/.app.MainActivity`. Existing app data was retained. |
| 2026-10-25 | Appearance controls and persistence, Android API 36 | PASS | Selected Light and Dark with Dynamic color both on and off. Verified checked UI semantics and only the two appearance preference values. Force-stopped and relaunched the app for each combination; selections persisted and Settings remained usable. No provider probe or message was sent. |
| 2026-10-25 | System mode and system-bar icon flags, Android API 36 | PASS | Selected System and changed Android night mode from yes to no. Verified `LIGHT_STATUS_BARS` and `LIGHT_NAVIGATION_BARS` were set for Light (dark icons) and absent for Dark (light icons). Verified forced Dark overrides light Android and forced Light overrides dark Android, including process restarts. Waited for checked radio semantics before querying window flags; an initial immediate post-tap assertion raced recomposition. Restored System, Dynamic color on, and Android night mode yes. |
| 2026-10-25 | Palette fidelity, chat/markdown/tool/diff contrast, safe insets and themed launcher icon | PENDING | Manual visual inspection still required: compare System/Light/Dark with dynamic color on/off; inspect links, fenced/inline code, bash output, errors and added/removed diff lines; open the keyboard and check system-bar/IME insets; enable themed launcher icons. No screenshots were captured. |
| 2026-10-25 | Pre-31 static fallback and hidden Dynamic color switch | PENDING | On an API 29 or 30 device, verify the Dynamic color control is absent and System/Light/Dark use the explicit static palettes. The connected phone is API 36 only. |

## Stage 1 bash hardening - 2026-10-25

- DONE: workspace fmt and Clippy (`--workspace --all-targets -- -D warnings`),
  plus `cargo deny check` (advisories, bans, licenses, sources).
- PARTIAL: workspace tests were split to cap exec separately. The non-exec
  run passed 155 tests, with one ignored subprocess helper and one failure:
  `tools::bash::tests::timeout_kills_the_tree_fast` exceeded its 10-second
  assertion on this Windows sandbox (53.81 seconds for the tools suite).
  The other 68 tools tests passed. This is not a green workspace test gate.
- UNRUN: Windows exec integration suite, capped at 60 seconds with
  `--test-threads=1`; cancellation/process-tree checks stall here. The
  stage-end attempt also waited for the build lock. Four exec unit tests
  passed. Run the complete exec suite on an unrestricted host.
- DONE: the host subprocess regression killed an engine during a real
  HostExecutor bash call, restarted against the same database, checked
  `interrupted, outcome unknown` in model history, and resumed against a
  local mock. The command's start marker remained exactly one line and no
  new ToolStarted event occurred. Timer and final BashTap flush tests passed.
- DONE: attached Android API 36 phone, text-only adb: 5 exec unit tests and
  15 integration tests passed, including post-EOF timeout, successful-exit
  process-group cleanup, adopted setsid children, identity-checked pidfiles,
  the 32 MiB cap/drain marker, slow-sink backpressure, and absence of the
  actual app credential variable names from child `env` output.
- DONE: 16 classifier tests and 3 userland tests passed on the same phone.
  These test sources were linked as standalone Rust test harnesses against
  the built crates. Coverage includes canonical symlink containment,
  secret names, flag values, literal expansion syntax, injected lookup /
  mkdir / link / executable failures, stale links and directory obstructions.
  Temporary device test executables were removed.
- DONE: `:android:app:assembleDebug` built both Android ABIs and Kotlin
  bindings. The universal debug APK installed with `adb install -r` and
  cold-launched via `am start -W`; the process remained live and the app's
  sh/rg/git farm links pointed to the newly installed native directory.
- PENDING: in-app injected-failure result presentation, kill-mid-command
  recovery of the full Android app, screen-off survival, and the remaining
  existing M2/M3 acceptance checks. No screenshots, UI tapping or live
  gateway requests were used. Full M2/M3 acceptance is not claimed.
- Risks: bash remains same-UID execution, not confinement. ADR 0017 records
  serialized Linux/Android cleanup and the startup detached-child identity
  gap. Windows process-control verification remains outstanding. Deferred
  history, vendor, F-Droid, M4, HTTPS git and iOS work was not changed.
  Local commit hooks could not run gitleaks because it was absent from PATH;
  no history scan was attempted.
  Earlier sandbox crash/stage test logs remain untracked because orphan
  shell handles deny deletion. The crash helper now waits for its release
  acknowledgement before deleting its directory; the regression passed
  again after that cleanup fix.

## Stage 2 concurrency (2026-10-25)

- DONE: bash executions share a four-slot gate across sessions; queued
  cancellation does not spawn a child. Two queue regressions pass.
- DONE: tools fmt and clippy with warnings denied; 66 tools tests pass with
  the five process-executing bash tests excluded.
- UNRUN: the full tools test run stalled in process execution in this
  sandbox and was stopped. No Android behavior was exercised.
- PENDING: device checks remain with the maintainer.
