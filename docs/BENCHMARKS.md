# BENCHMARKS

Budgets and measured actuals. Nothing measured yet — M1 records the first
per-turn overhead number; M6 publishes the full table.

| Budget | Target | Measured | Device | Date |
|--------|--------|----------|--------|------|
| Harness overhead per turn (excl. tool exec) | < 50 ms | — | — | — |
| `read` tool | < 10 ms | — | — | — |
| `grep`, ~10k-file repo, warm | < 300 ms | — | — | — |
| `bash` spawn | < 30 ms | **~8 ms** (exec-probe, single sample, cold app) | Emulator palmcode34 x86_64 API 34 | 2026-10-10 |
| Streaming jank @100 tok/s | < 2% janky frames | — | — | — |
| Core RSS idle, 1k-file workspace | < 150 MB | — | — | — |
| APK size arm64 | < 30 MB | **22.04 MB release unsigned** (+ x86_64 22.04 MB, universal 22.47 MB; splits enabled) | host | 2026-10-11 |
| Userland binaries (stripped, measured) | inform APK budget | git **3.55 MB** arm64; ripgrep 15.2.0 **4.21 MB** arm64 / 4.50 MB x86_64; busybox 1.38.0 **0.945 MB** arm64 / 0.941 MB x86_64 | NDK 30.0.16248370; `userland/spike/*-build.sh` | 2026-10-11 |
| LLM streaming (Grid gateway, glm-latest) | observe | TTFT **~0.8 s**, **~70 tok/s**, max idle gap 76 ms; `reasoning_content`; usage final-chunk-only; prefix-cache counters not exposed; `/tokenize` 403 → heuristic fallback | `scripts/probe-vllm.ps1` | 2026-10-11 |
| Cold start to interactive | < 1.2 s | — | — | — |
