package dev.palmcode.bench

import android.app.Instrumentation
import android.os.Bundle
import dev.palmcode.app.ProviderSettings
import dev.palmcode.app.ProviderStore

/** Synthetic credentials in dedicated bench-only preferences; never touches the main app. */
class ProviderStoreInstrumentation : Instrumentation() {
    private var mode = "write"
    override fun onCreate(arguments: Bundle?) {
        super.onCreate(arguments)
        mode = arguments?.getString("mode") ?: "write"
        start()
    }

    override fun onStart() {
        val name = "provider-save-regression"
        val fixture = ByteArray(24) { (it + 65).toByte() }
        val results = Bundle()
        try {
            val store = ProviderStore(targetContext, name)
            if (mode == "write") {
                store.save(ProviderSettings("http://127.0.0.1:1/v1", ""), fixture)
                check(store.hasApiKey && !store.configured)
                results.putBoolean("credentials_saved_without_model", true)
            } else {
                check(store.hasApiKey)
                val loaded = store.loadApiKey()
                try { check(loaded != null && fixture.contentEquals(loaded)) }
                finally { loaded?.fill(0) }
                results.putBoolean("key_survived_process_restart", true)
                store.save(ProviderSettings("http://127.0.0.1:1/v1", "fixture"))
                val kept = ProviderStore(targetContext, name).loadApiKey()
                try { check(kept != null && fixture.contentEquals(kept)) }
                finally { kept?.fill(0) }
                results.putBoolean("blank_keeps_key", true)
                fixture.reverse()
                store.save(ProviderSettings("http://127.0.0.1:1/v1", "fixture"), fixture)
                val replaced = ProviderStore(targetContext, name).loadApiKey()
                try { check(replaced != null && fixture.contentEquals(replaced)) }
                finally { replaced?.fill(0) }
                results.putBoolean("replacement_persisted", true)
                check(targetContext.getSharedPreferences(name, 0).edit().clear().commit())
            }
            finish(-1, results)
        } catch (_: Exception) {
            results.putBoolean("provider_save_regression_failed", true)
            finish(1, results)
        } finally {
            fixture.fill(0)
        }
    }
}
