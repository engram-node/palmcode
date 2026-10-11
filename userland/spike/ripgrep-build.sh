#!/usr/bin/env bash
# Spike: ripgrep 15.2.0 cross-build for Android (item 9 size measurement).
# Pure-Rust dependency closure (pcre2 off by default) -> linker is all we need.
set -euo pipefail

VER=15.2.0
NDK_BIN="/c/Users/OM/AppData/Local/Android/Sdk/ndk/30.0.16248370/toolchains/llvm/prebuilt/windows-x86_64/bin"
W="/home/OM/palmcode-spike/ripgrep"
mkdir -p "$W" && cd "$W"
export PATH="$NDK_BIN:/c/Users/OM/.cargo/bin:/c/Users/OM/.rustup/toolchains/1.99.0-x86_64-pc-windows-gnu/bin:$PATH"

TARBALL=ripgrep-$VER.tar.gz
[ -f "$TARBALL" ] || curl -fSL -o "$TARBALL" "https://github.com/BurntSushi/ripgrep/archive/refs/tags/$VER.tar.gz"
echo "== tarball sha256 (record into versions.env):"
sha256sum "$TARBALL"
# Windows tar cannot create the HomebrewFormula symlink (needs admin);
# harmless for our build, so tolerate tar's failure if the sources landed.
if [ ! -d "ripgrep-$VER/crates" ]; then tar xf "$TARBALL" || true; fi
[ -d "ripgrep-$VER/crates" ] || { echo "extract failed"; exit 1; }
cd "ripgrep-$VER"

for T in aarch64-linux-android x86_64-linux-android; do
  case $T in
    aarch64*) TARGET_CLANG="--target=aarch64-linux-android29" ; OUT=rg-arm64 ;;
    x86_64*)  TARGET_CLANG="--target=x86_64-linux-android29"  ; OUT=rg-x64 ;;
  esac
  rustup target add "$T" >/dev/null
  echo "== building ripgrep $VER for $T"
  CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER="$NDK_BIN/clang.exe" \
  CARGO_TARGET_X86_64_LINUX_ANDROID_LINKER="$NDK_BIN/clang.exe" \
  RUSTFLAGS="-C link-arg=$TARGET_CLANG" \
    cargo.exe build --release --target "$T"
  llvm-strip "target/$T/release/rg" -o "$OUT"
  echo "== $T size:"
  ls -l --block-size=1 "$OUT" | awk '{print $5, $9}'
  file "$OUT"
done
echo "== ripgrep spike done"
