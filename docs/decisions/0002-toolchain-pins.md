# 0002: Toolchain and dependency pins (M0)

Date: 2026-10-10. Status: accepted.

Latest stable at pin time, verified against primary sources:

| Tool | Version | Source |
|------|---------|--------|
| Rust | 1.99.0 | rustup stable 2026-09-28 |
| Android Gradle Plugin | 9.4.0 | developer.android.com (2026-10-01) |
| Gradle | 9.7.1 | gradle.org (2026-08-19) |
| Kotlin (compose compiler plugin) | 2.4.21 | kotlinlang.org (2026-10-08) |
| Android NDK | 30.0.16248370 (r30 LTS) | developer.android.com/ndk (2026-09-08) |
| compileSdk / targetSdk / minSdk | 36 / 36 / 29 | Android 16 stable |
| Compose BOM | 2026.09.00 | mvnrepository (2026-09-09) |
| UniFFI | 0.32.2 | crates.io (2026-09-23) |
| cargo-ndk | 4.1.2 | crates.io (2025-08-09) |
| JNA | 5.19.1 (@aar) | Maven Central (2026-06-12) |

## Notable choices

- **Gradle 9.7.1, not 9.8.1**: 9.8.0/9.8.1 shipped the same week (2026-10-07/09)
  with zero soak time; 9.7.1 satisfies AGP 9.4's minimum (9.6). Revisit at M5.
- **NDK r30 (LTS)** over r29: 16 KB page alignment is default since r28;
  r30 is the supported LTS well past F-Droid submission.
- **compileSdk/targetSdk 36** (Android 16 stable). Android 17 is in beta;
  `adb`/platform for 16 is installed. Bump when 17 stabilizes.
- **minSdk 29** per the project brief (exec rules differ below 29; PIE).
- `android.useAndroidX=true` default; `android.nonTransitiveRClass=true`.

Re-pin policy: bump at each milestone gate, record the reason here amending
this ADR.
