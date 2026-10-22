# palmcode

On-device, BYOK coding-agent harness for Android. The agent runs entirely on the
phone; the only network traffic is the user's own LLM requests to an
vLLM-serving endpoint they configure (docs/VLLM.md). No backend, no accounts, no
telemetry.

Distributed via F-Droid. License: Apache-2.0.

## Status

**Milestone 0 (skeleton): complete** â€” repo layout, pinned toolchains, UniFFI
FFI path proven on an emulator, cargo-ndk build wired into Gradle, exec-via-
symlink spike passed, git cross-build spike passed. Both risk spikes green;
see `docs/VERIFY.md` and `docs/decisions/` (0001â€“0007). M1 (provider + loop)
up next.

## Build

Prerequisites (exact versions pinned in `docs/decisions/0002-toolchain-pins.md`):

- JDK 21 (Temurin), `JAVA_HOME` set
- Rust via rustup (`rust-toolchain.toml` pins the toolchain and targets)
- `cargo install --locked cargo-ndk`
- Android SDK with `platforms;android-36`, `build-tools;36.0.0`, `ndk;30.0.16248370`
  (`sdk.dir` in `local.properties` or `ANDROID_HOME`)

```
./gradlew assembleRelease
```

Gradle drives `cargo ndk`, binding generation, and (from M3) the userland
build. The Rust build vendors its dependencies (`core/vendor/`), so it works
offline after a clean checkout.

## Secrets policy

This repo must stay F-Droid-publishable: no API keys, no provider hostnames
or model aliases anywhere in the tree or docs (see
`docs/decisions/0013-secrets-and-provider-neutrality.md`). Endpoint, key and
model come from env only: `PALMCODE_LLM_BASE_URL`, `PALMCODE_LLM_API_KEY`,
`PALMCODE_LLM_MODEL` (`PALMCODE_LLM_FAST_MODEL` for the fast role).

CI runs gitleaks on every push/PR and fails on findings. For local
pre-commit scanning (gitleaks >= 8.19 on PATH):

```
git config core.hooksPath .githooks
```

## Layout

```
core/       Rust workspace (agent, llm, tools, exec, store, ffi, host-cli, bench)
android/    Kotlin + Jetpack Compose app
userland/   reproducible source builds for busybox, ripgrep, git
docs/       VERIFY, ADRs, F-Droid metadata draft
fastlane/   store metadata (en-US)
```
