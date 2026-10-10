#!/usr/bin/env bash
# Spike A, attempt 2: compile git for both ABIs after the successful configure
# (aarch64 = product target; x86_64 = fresh extract, runs on the emulator).
# Note: git's `make distclean` DELETES configure — only ever re-extract from
# the tarball, never distclean a configured copy.
set -euo pipefail
N=/c/Users/OM/AppData/Local/Android/Sdk/ndk/30.0.16248370/toolchains/llvm/prebuilt/windows-x86_64
export PATH="$N/bin:$PATH"

gitconf() {
  local triple=$1
  ./configure --host="$triple" \
    CC="clang --target=${triple}29" \
    AR=llvm-ar RANLIB=llvm-ranlib STRIP=llvm-strip \
    --without-iconv --without-curl --without-expat --without-tcltk \
    --without-python --without-libpcre2 \
    ac_cv_snprintf_returns_bogus=no \
    ac_cv_fread_reads_directories=no >/dev/null
}

echo "=== arm64: reuse configured tree ==="
cd ~/pocket-spike/git-2.56.0
cat > config.mak <<'EOF'
HAVE_DEV_TTY=YesPlease
# git 2.56 builds its optional Rust component by default (needs a working
# cargo for the target); the C core is the full product, so disable it.
NO_RUST=YesPlease
# bionic has pthreads but no pthread cancellation API; threads are never
# cancelled anyway, so stubbing setcancelstate to success is semantically OK.
BASIC_CFLAGS += -DPTHREAD_CANCEL_DISABLE=0 "-Dpthread_setcancelstate(a,b)=0"
EOF
make -j"$(nproc)" git 2>&1 | tail -4
ls -la git && file git

echo "=== x86_64: fresh extract ==="
X=~/pocket-spike/git-x86_64
rm -rf "$X"; mkdir -p "$X"
tar -C "$X" --strip-components=1 -xf ~/pocket-spike/git.tar.xz
cd "$X"
gitconf x86_64-linux-android
cat > config.mak <<'EOF'
HAVE_DEV_TTY=YesPlease
NO_RUST=YesPlease
BASIC_CFLAGS += -DPTHREAD_CANCEL_DISABLE=0 "-Dpthread_setcancelstate(a,b)=0"
EOF
make -j"$(nproc)" git 2>&1 | tail -4
ls -la git && file git
