# F-Droid scanner report

Tool: `fdroidserver 2.4.5` (pip, isolated venv), SuSi signature rules.
Run 2026-10-12 on the m1-prep tree (== #5 head) and on
`android/app/build/outputs/apk/release/app-arm64-v8a-release-unsigned.apk`.

Reproduce:

```powershell
python -m venv <venv>; <venv>/Scripts/pip install fdroidserver
<venv>/Scripts/fdroid scanner --verbose <apk>          # APK scan
# source tree scan: call fdroidserver.scanner.scan_source(build_dir, build)
# (the CLI wants fdroiddata metadata; the API path is what build entries use)
```

## APK scan — CLEAN

- `classes2.dex` scanned with dexdump for known non-free classes: none found.
- Extra signing blocks: none suspicious.

## Source tree scan — 0 errors

`scan_source` over the whole checkout: **errors: []**, 9 infos, 9093 warnings.

**Infos (9)** — scanner's own safe-removal rules, all expected:
- `gradlew`, `gradlew.bat`, `gradle/wrapper/gradle-wrapper.jar` — the one
  sanctioned jar; F-Droid builds replace gradle setup anyway.
- 6 × built APKs under `android/app/build/outputs/` — local build output,
  git-untracked.

**Warnings (9093)** — "executable binary, possibly code":
- 8990 on git-**untracked** build output (`.gradle/`, `android/app/build/intermediates/`)
  — never part of the repo.
- **103 on tracked files, all inside `core/vendor/` vendored crates:**
  | Count | Crate | Content |
  |---|---|---|
  | 29 | ring | `pregenerated/*.o` NASM objects (MSVC-only; our gnu/android targets compile ring from `.S`/C), test `.der` keys |
  | 26 | rustls-platform-verifier | test `.ocsp`/`.der` fixtures |
  | 19 | rustls-pki-types | test `.der` fixtures |
  | 13 | openssl | test `.der` fixtures |
  | 4+3 | tokio-native-tls, rustls-webpki | test cert fixtures |
  | 6 | windows_*_msvc×3 | MSVC import `.lib`s (upstream-crate content; unused on our gnu/ELF targets) |
  | 2+1 | unicode-ident, askama_parser | `.fst`/test fixtures |

All are upstream crate content distributed via crates.io and pinned by
`.cargo-checksum.json`; none is a userland binary we introduced.
AGENTS.md's binary rule targets *our* binaries; vendored source is the
sanctioned exception, exactly as ring's pregenerated objects already were.

## The vendored verifier AAR — NOT flagged

`core/vendor/rustls-platform-verifier-android/maven/.../rustls-platform-verifier-0.1.1.aar`
produced **neither an error nor a warning** from the scanner (its rules
trigger on `.jar`/executables/shared objects, not on raw `.aar` zips in
source trees) and its classes are our own Apache-2.0 MIT companion code
inside the APK (DEX scan clean). Verdict: **would not fail F-Droid as-is**;
no fix required now. The contingency (rebuild the AAR from upstream Kotlin
sources inside the Gradle build) stays documented in
`docs/decisions/0010-tls-split-and-verifier-aar.md` if a future scanner
release tightens `.aar` rules.
