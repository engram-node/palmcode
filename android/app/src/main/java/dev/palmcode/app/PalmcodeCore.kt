package dev.palmcode.app

import android.content.Context

/**
 * Loads the Rust core and owns its JNI entry point. UniFFI functions come
 * from the same library once this object has been initialized.
 */
object PalmcodeCore {
    init {
        System.loadLibrary("palmcode_ffi")
    }

    /**
     * One-time Android TLS init: rustls-platform-verifier needs the app
     * Context to verify certificates against the platform trust store.
     * Returns 0 on success (Rust side logs details on failure).
     */
    @JvmStatic
    external fun nativeInitAndroid(context: Context): Int
}
