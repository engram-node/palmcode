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
| LLM streaming (Grid gateway, glm-latest) | observe | TTFT **~0.8 s**, **~70 tok/s**, max idle gap 76 ms; `reasoning_content`; usage final-chunk-only; prefix-cache counters not exposed; `/tokenize` 403 → heuristic fallback | `scripts/probe-vllm.ps1` | 2026-10-11 |
| Cold start to interactive | < 1.2 s | **1038 ms** (`am start -W` TotalTime, debug APK; includes core load + SQLite + Keystore decrypt) | motorola edge 50 neo | 2026-10-11 |
| Cold start (release/R8) | < 1.2 s | **316–463 ms** across 4 runs (`am start -W` TotalTime; first 463, then 331/316/301; sessions + Keystore intact across debug->release install) | motorola edge 50 neo | 2026-10-12 |
| App RSS idle (M1 session list) | inform 150 MB core budget | **native heap 21 MB**, total PSS 150 MB/RSS 251 MB (Compose+ART included) | motorola edge 50 neo | 2026-10-11 |
