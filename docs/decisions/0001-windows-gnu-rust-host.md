# 0001: Rust host toolchain is `x86_64-pc-windows-gnu`

Date: 2026-10-10. Status: accepted.

## Context

The dev machine (Windows 11) has no Visual Studio / Build Tools installed, so
the default MSVC host target cannot link anything (proc macros and build
scripts included). Installing VS Build Tools costs several GB and an admin
install.

## Decision

Use `x86_64-pc-windows-gnu` as the host triple (bundles its own MinGW linker).
`default-host` set at rustup install time. Android targets are unaffected;
CI builds host-side code on `x86_64-unknown-linux-gnu` (glibc) anyway.

## Alternatives rejected

- Install VS Build Tools: heavy, adds nothing for an Android-targeted core.
- WSL2 for all Rust: file-system crossing would slow the cargo/gradle loop.

## Consequences

- If a future desktop host CLI on Windows needs MSVC-only crates (e.g. some
  `windows-sys`-binding crates are fine, but anything requiring `link.exe`
  extensions), revisit.
- UniFFI bindgen runs against the compiled Android `libpocketffi.so`
  (ELF/goblin), sidestepping host-format concerns entirely.
- **raw-dylib caveat (found on this machine):** crates using
  `#[link(kind="raw-dylib")]` (getrandom 0.3 via windows-link) require a
  working `dlltool`+`as` pair. rust-mingw's self-contained `dlltool.exe` ships
  no assembler and fails with "dlltool.exe: CreateProcess"; an ancient
  `C:\MinGW\bin\dlltool.exe` also on this box fails with "Invalid bfd target".
  Fix in place here: self-contained `dlltool.exe` renamed to
  `dlltool.exe.disabled`, MSYS2 `C:\msys64\mingw64\bin` (binutils 2.44)
  prepended to PATH for all cargo invocations (shell and Gradle Exec tasks).
  Linux CI is unaffected.
