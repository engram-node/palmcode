# 0003: AGP 9 with built-in Kotlin (no KGP) and Compose compiler plugin

Date: 2026-10-10. Status: accepted.

## Decision

Use AGP 9.4's built-in Kotlin support: do **not** apply
`org.jetbrains.kotlin.android`. Apply only
`org.jetbrains.kotlin.plugin.compose` (Kotlin 2.4.21) for the Compose compiler.

## Alternatives rejected

- AGP 8.13 + classic KGP: older stack, and AGP 9 is the supported default for
  new projects; migrating later is churn we can skip by starting on 9.
- Opting out via `android.builtInKotlin=false`: removal promised in AGP 10.

## Notes

- Greenfield project: none of the migration hazards (kotlinOptions,
  android.sourceSets kotlin dirs) apply; generated sources are added through
  `android.sourceSets["main"].kotlin.srcDir(...)`.
- Kotlin compiler version for app sources is whatever AGP 9.4 ships embedded;
  the compose-compiler plugin version must stay compatible (2.4.21 declared
  compatible with AGP 9.1+ fixes; verify at build time).
