# UniFFI calls into Rust via JNA; keep JNA API intact if minify is enabled.
-keep class com.sun.jna.** { *; }
-keep class dev.palmcode.core.** { *; }
# Rust calls this handler by exact JNI name.
-keep class dev.palmcode.app.PalmcodeCore { *; }
# rustls-platform-verifier looks these up by exact JNI name from Rust.
-keep class org.rustls.platformverifier.** { *; }

# JNA carries unused desktop entries; never reachable on Android (R8-generated list).
-dontwarn java.awt.Component
-dontwarn java.awt.GraphicsEnvironment
-dontwarn java.awt.HeadlessException
-dontwarn java.awt.Window
