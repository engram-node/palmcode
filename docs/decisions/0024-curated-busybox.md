# 0024: Compile only reviewed BusyBox applets

Replace defconfig-minus-a-blacklist with allnoconfig plus an explicit
125-applet Kconfig allowlist. A blacklist can silently acquire new upstream
administrative applets. Keep shell, file/text processing, compression,
process inspection and common build helpers; omit boot, mount, module,
login and system-service tools. This curates the binary, not only PATH.

The build compares each ABI's generated dispatch table with the allowlist
and Rust catalog and checks the 100-150 bound. Generate the prompt catalog
with userland/generate-catalog.sh. Gradle tracks both inputs so cached output
cannot hide a changed selection. Source checksum verification is unchanged.

Catalog estimate (characters divided by four, rounded up; not a tokenizer):
367 applets / 719 tokens before; 125 applets / 205 tokens after. This estimate
covers only the comma-separated applet list, not the complete system prompt.

Stripped native sizes in bytes:

| ABI | Before | After |
| --- | ---: | ---: |
| arm64-v8a | 990968 | 338944 |
| x86_64 | 986784 | 333760 |

Both ABI dispatch-table/catalog checks passed during the source build.
Runtime behavior of the reduced feature set remains a device acceptance
check. The debug universal APK comparison is recorded in docs/VERIFY.md.
