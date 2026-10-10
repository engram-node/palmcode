#!/usr/bin/env bash
# Reproduce the arm64 compile error verbosely.
N=/c/Users/OM/AppData/Local/Android/Sdk/ndk/30.0.16248370/toolchains/llvm/prebuilt/windows-x86_64
export PATH="$N/bin:$PATH"
cd ~/pocket-spike/git-2.56.0
make run-command.o V=1 2>&1 | tail -25
