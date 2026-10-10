#!/usr/bin/env bash
# Spike A, attempt 1: upstream ./configure cross-compile for Android.
# Run under MSYS2:  bash /c/repos/palmcode/userland/spike/git-configure.sh
set -euo pipefail
cd ~/palmcode-spike/git-2.56.0
N=/c/Users/OM/AppData/Local/Android/Sdk/ndk/30.0.16248370/toolchains/llvm/prebuilt/windows-x86_64
export PATH="$N/bin:$PATH"
./configure --host=aarch64-linux-android \
  CC="clang --target=aarch64-linux-android29" \
  AR=llvm-ar RANLIB=llvm-ranlib STRIP=llvm-strip \
  --without-iconv --without-curl --without-expat --without-tcltk \
  --without-python --without-libpcre2 \
  ac_cv_snprintf_returns_bogus=no \
  ac_cv_fread_reads_directories=no 2>&1 | tail -25
