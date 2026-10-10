# UniFFI calls into Rust via JNA; keep JNA API intact if minify is enabled.
-keep class com.sun.jna.** { *; }
-keep class dev.palmcode.core.** { *; }
