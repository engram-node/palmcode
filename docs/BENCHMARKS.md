# BENCHMARKS

Budgets and measured actuals. Nothing measured yet — M1 records the first
per-turn overhead number; M6 publishes the full table.

| Budget | Target | Measured | Device | Date |
|--------|--------|----------|--------|------|
| Harness overhead per turn (excl. tool exec) | < 50 ms | **p50/p95 0.9 ms** (25 turns, long_prefill fixture ×71 chunks; wall minus mock's own measured stream span — Windows tokio timer slop makes the scripted delay unusable) | host Windows, release build | 2026-10-11 |
| `read` tool | < 10 ms | — | — | — |
| `grep`, ~10k-file repo, warm | < 300 ms | — | — | — |
| `bash` spawn | < 30 ms | **~8 ms** (exec-probe, single sample, cold app) | Emulator palmcode34 x86_64 API 34 | 2026-10-10 |
| Streaming jank @100 tok/s | < 2% janky frames | **0.40%** (13/3263, `dumpsys gfxinfo` after `reset`, 20 s window mid-stream vs `bench serve 100` over `adb reverse`) | motorola edge 50 neo (Android 16) | 2026-10-11 |
| Streaming jank @100 tok/s (release/R8) | < 2% | **0.00%** (0/7310 frames, 64 s window mid-stream; "Stop" still offered at end of window; stop->mock observed disconnect; R8 baseline-profile build renders ~114 fps avg on 120 Hz panel) | motorola edge 50 neo (Android 16) | 2026-10-12 |
| Core RSS idle, 1k-file workspace | < 150 MB | — | — | — |
| APK size arm64 | < 30 MB target / 40 MB hard | **5.14 MB release unsigned with R8+shrinkResources** (was 25.74 MB unshrunk at M1). Composition: `libpalmcode_ffi.so` 2.70 MB (5.69 raw), DEX 2.47 MB, resources.arsc 0.10 MB, libjnidispatch 0.04 MB, graphics-path/execprobe/META-INF/kotlin-meta/res/assets each < 0.05 MB. Pre-R8 composition was 88% dead DEX (classes2 stored uncompressed 23.66 MB). Keep rules: JNA, uniffi gen, PalmcodeCore JNI, `org.rustls.platformverifier.**` (missing-keep caused all-HTTPS failure, caught by probe) | host | 2026-10-12 |
| Userland binaries (stripped, measured) | inform APK budget | git **3.55 MB** arm64; ripgrep 15.2.0 **4.21 MB** arm64 / 4.50 MB x86_64; busybox 1.38.0 **0.945 MB** arm64 / 0.941 MB x86_64 | NDK 30.0.16248370; `userland/spike/*-build.sh` | 2026-10-11 |
| `libpalmcode_ffi.so` arm64 release raw | inform APK budget | M2 pre-tree-sitter **8.25 MB** (8,253,912 B); with 9 tree-sitter grammars (rust/py/js/ts/tsx/json/c/cpp/go/java, runtime 0.25) **17.83 MB** (17,830,704 B) → tree-sitter delta **+9.13 MB** (~1.0 MB/grammar). APK impact = compressed portion of the .so; budget ok (< 30 MB) | host, NDK 30.0.16248370, `cargo ndk -t aarch64-linux-android --release` | 2026-10-12 |
| LLM streaming (Grid gateway, glm-latest) | observe | TTFT **~0.8 s**, **~70 tok/s**, max idle gap 76 ms; `reasoning_content`; usage final-chunk-only; prefix-cache counters not exposed; `/tokenize` 403 → heuristic fallback | `scripts/probe-vllm.ps1` | 2026-10-11 |
| Cold start to interactive | < 1.2 s | **1038 ms** (`am start -W` TotalTime, debug APK; includes core load + SQLite + Keystore decrypt) | motorola edge 50 neo | 2026-10-11 |
| Cold start (release/R8) | < 1.2 s | **316–463 ms** across 4 runs (`am start -W` TotalTime; first 463, then 331/316/301; sessions + Keystore intact across debug->release install) | motorola edge 50 neo | 2026-10-12 |
| Cold start (release/R8, M2 w/ tree-sitter .so) | < 1.2 s | **345–537 ms** across 4 runs (537 first, 389/346/345 after; +~35–75 ms vs M1 for the +9 MB raw grammars) | motorola edge 50 neo | 2026-10-13 |
| APK size arm64 (M2) | < 30 MB target | **7.24 MB** release unsigned (M1: 5.14 MB; +2.1 MB compressed = tree-sitter grammars + tool code) | host `assembleRelease` | 2026-10-13 |

### Release APK composition & the 22/25.7 → 5.1/7.2 MB story (arm64 unsigned, 2026-10-13)

Why it shrank: the M0/M1 APKs (~22 MB / 25.74 MB unshrunk) were ~88% dead
DEX — `classes2.dex` of 23.66 MB sat in the APK uncompressed. M1 enabled R8
full-mode + `shrinkResources` (PR #5): DEX collapsed to ~2.4 MB and the APK
to 5.14 MB. M2 grew it back to 7.24 MB by adding 9 tree-sitter grammars
(+9.13 MB raw inside the Rust `.so`, ~+2.1 MB after deflate) plus tool code.

| Entry | In-APK (deflate) | Raw | What it is |
|---|---:|---:|---|
| `lib/arm64-v8a/libpalmcode_ffi.so` | **4.61 MB** | 17.01 MB | Rust core: ffi + agent + llm + tools + store + sqlite + blahaj + 9 tree-sitter grammars |
| `classes.dex` | **2.42 MB** (stored) | 2.42 MB | Kotlin/Compose app, R8-shrunk |
| `resources.arsc` | 0.09 MB | 0.09 MB | compiled resources |
| `lib/arm64-v8a/libjnidispatch.so` | 0.038 MB | 0.162 MB | JNA (SQLite) |
| licenses/assets/manifest/prof | < 0.05 MB | — | THIRD_PARTY_LICENSES, baseline.prof, etc. |
| **userland (busybox/rg/git)** | **0 — NOT packaged yet** | — | built + device-verified in M1 spikes; packaging lands with `userland/build.sh` (below) |
| **Total** | **7.24 MB** | 19.88 MB | hard limit 40 MB → **~32 MB headroom** |

Userland size forecast (stripped aarch64, from device-verified spike builds):
git 3.55 MB + ripgrep ~2.7 MB + busybox ~1.1 MB ≈ **+7.4 MB raw → ~+3.5–4 MB
in-APK** per ABI → arm64 APK ≈ 11 MB. x86_64 build doubles only the
per-ABI entries in the universal APK.
| M2 tool latency on device | read < 10 ms / ls < 300 ms-class | `read` **3 ms**, `grep` **7 ms**, `edit` **0–5 ms**, `ls src` **8 ms** (Grid tool cards, demo workspace) | motorola edge 50 neo | 2026-10-13 |
| edit-bench (K) vs Grid, glm-5.3-flash | metrics run | **18/20 pass (90%)**, mean **12.3 s**/task, **98 tool calls (1 tool error)**, tokens 25 895 in / 5 448 out. Fails are model-level: `rs-extract-struct` chose `&str` over `String`; `js-const` kept a `3.14159` occurrence. | host `cargo run -p palmcode-bench --bin edit-bench` | 2026-10-13 |
| App RSS idle (M1 session list) | inform 150 MB core budget | **native heap 21 MB**, total PSS 150 MB/RSS 251 MB (Compose+ART included) | motorola edge 50 neo | 2026-10-11 |
