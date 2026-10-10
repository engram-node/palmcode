# TROUBLESHOOTING

Growing list; entries are added as they are confirmed on hardware.

## cargo fails compiling `windows-sys`/`getrandom` on Windows (host builds)

Symptoms: "Invalid bfd target" or "dlltool.exe: CreateProcess" from a
`dlltool` invocation. Cause: raw-dylib crates (windows-sys / getrandom 0.3)
run `dlltool`, and the machine's PATH resolves it to a stale MinGW or to
rust-mingw's assembler-less self-contained stub. Fix: install MSYS2 binutils
(`pacman -S mingw-w64-x86_64-binutils`), rename the self-contained stub
(`.../self-contained/dlltool.exe` → `dlltool.exe.disabled`), and keep
`C:\msys64\mingw64\bin` first in PATH for cargo runs (Gradle Exec tasks do
this automatically). Details: docs/decisions/0001.

## Child process killed without error (Android 12+, "phantom process killer")

Android limits child processes per app and may kill them under memory
pressure. Symptoms: bash tool processes exit silently near the 4-process
concurrency cap.

Non-root mitigation (needs adb once per boot on some builds):

```
adb shell "settings put global settings_enable_monitor_phantom_procs false"
```

Rooted devices:

```
adb shell su 0 setprop persist.sys.fflag.override.settings_enable_monitor_phantom_procs false
```

palmcode additionally caps concurrent child processes (default 4) and reaps
aggressively.

## "exec failed: EACCES (Permission denied)" from the bash tool (M3+)

Binaries must be shipped as `lib<name>.so` and require
`packaging.jniLibs.useLegacyPackaging=true`. If this regresses after an AGP
bump, check that install-time extraction still happens
(`run-as dev.palmcode ls -l <nativeLibraryDir>` in a debug build).

## TLS errors to self-hosted endpoints

palmcode uses the platform trust store. Install your private CA via Android
settings; user CAs are honored for apps that trust them (we do; M1).

## cargo: "listed checksum of core/vendor/... has changed" (Windows checkouts)

Cause: git line-ending normalization altering vendored bytes. Each vendored
crates carries a `.cargo-checksum.json` keyed to the *published* bytes, and
some upstream files contain CRLF. If git checks those files out as LF (e.g.
`core.autocrlf=true` on Windows, or a global `* text=auto` rule), the bytes
change and cargo aborts.

Fix in this repo (already applied): `.gitattributes` pins
`core/vendor/** -text` so vendored content round-trips byte-identically on
every OS. If you add another vendored tree, add its own `-text` rule, and
leave `core.autocrlf` unset for this repo (`git config core.autocrlf false`
locally if you keep it `true` globally).
