package dev.palmcode.app

import android.content.SharedPreferences
import java.lang.reflect.Proxy
import javax.crypto.Cipher
import javax.crypto.spec.GCMParameterSpec
import javax.crypto.spec.SecretKeySpec
import org.junit.Assert.*
import org.junit.Test

class ProviderStoreTest {
    private class MemoryPreferences {
        val values = mutableMapOf<String, Any>()
        var commits = 0
        var failNextCommit = false
        val prefs: SharedPreferences = Proxy.newProxyInstance(
            SharedPreferences::class.java.classLoader, arrayOf(SharedPreferences::class.java),
        ) { _, method, args ->
            when (method.name) {
                "getAll" -> values.toMap()
                "contains" -> values.containsKey(args!![0])
                "edit" -> editor()
                "getString", "getInt", "getLong", "getBoolean", "getFloat", "getStringSet" ->
                    values[args!![0]] ?: args[1]
                else -> null
            }
        } as SharedPreferences

        private fun editor(): SharedPreferences.Editor {
            val pending = mutableMapOf<String, Any?>()
            lateinit var result: SharedPreferences.Editor
            result = Proxy.newProxyInstance(
                SharedPreferences.Editor::class.java.classLoader, arrayOf(SharedPreferences.Editor::class.java),
            ) { _, method, args ->
                when (method.name) {
                    "remove" -> { pending[args!![0] as String] = null; result }
                    "commit", "apply" -> {
                        pending.forEach { (key, value) ->
                            if (value == null) values.remove(key) else values[key] = value
                        }
                        commits++
                        val success = !failNextCommit
                        failNextCommit = false
                        if (method.name == "commit") success else null
                    }
                    else -> { pending[args!![0] as String] = args[1]; result }
                }
            } as SharedPreferences.Editor
            return result
        }
    }

    private val cipher = object : ApiKeyCipher {
        private val key = SecretKeySpec(ByteArray(32) { it.toByte() }, "AES")
        override fun encrypt(plain: ByteArray): SealedApiKey {
            val aes = Cipher.getInstance("AES/GCM/NoPadding")
            aes.init(Cipher.ENCRYPT_MODE, key)
            return SealedApiKey(aes.doFinal(plain), aes.iv)
        }
        override fun decrypt(sealed: SealedApiKey): ByteArray {
            val aes = Cipher.getInstance("AES/GCM/NoPadding")
            aes.init(Cipher.DECRYPT_MODE, key, GCMParameterSpec(128, sealed.iv))
            return aes.doFinal(sealed.blob)
        }
    }
    private val profile = ProviderSettings("http://127.0.0.1:1/v1", "")
    private val fixture = ByteArray(24) { (it + 65).toByte() }

    @Test fun encryptedKeyAndProfileSurviveReopenInOneCommit() {
        val memory = MemoryPreferences()
        val store = ProviderStore(memory.prefs, cipher)
        store.save(profile, fixture)
        assertEquals(1, memory.commits)
        assertTrue(store.hasApiKey)
        assertFalse(store.configured)
        assertFalse(memory.values.values.any { it.toString().contains(String(fixture)) })
        val reopened = ProviderStore(memory.prefs, cipher)
        assertArrayEquals(fixture, reopened.loadApiKey())
        assertEquals(profile.baseUrl, reopened.baseUrl)
    }

    @Test fun blankKeyKeepsEncryptedKeyWhenSettingsChange() {
        val memory = MemoryPreferences()
        val store = ProviderStore(memory.prefs, cipher)
        store.save(profile, fixture)
        val blob = memory.values["key_blob"]
        store.save(profile.copy(mainModel = "fixture"))
        assertEquals(blob, memory.values["key_blob"])
        assertArrayEquals(fixture, store.loadApiKey())
        assertTrue(store.configured)
    }

    @Test fun replacingKeyPersistsNewCiphertextAndKey() {
        val memory = MemoryPreferences()
        val store = ProviderStore(memory.prefs, cipher)
        store.save(profile, fixture)
        val oldBlob = memory.values["key_blob"]
        val replacement = fixture.reversedArray()
        store.save(profile, replacement)
        assertNotEquals(oldBlob, memory.values["key_blob"])
        assertArrayEquals(replacement, ProviderStore(memory.prefs, cipher).loadApiKey())
    }

    @Test fun encryptionFailureDoesNotSaveUrlOrReplaceExistingKey() {
        val memory = MemoryPreferences()
        ProviderStore(memory.prefs, cipher).save(profile, fixture)
        val before = memory.values.toMap()
        val failing = object : ApiKeyCipher {
            override fun encrypt(plain: ByteArray): SealedApiKey = error("Synthetic encryption failure")
            override fun decrypt(sealed: SealedApiKey) = cipher.decrypt(sealed)
        }
        assertThrows(IllegalStateException::class.java) {
            ProviderStore(memory.prefs, failing).save(profile.copy(baseUrl = "http://127.0.0.1:2/v1"), fixture)
        }
        assertEquals(before, memory.values)
        assertEquals(1, memory.commits)
    }

    @Test fun diskFailureRestoresCachedProfileAndKeyAndReportsFailure() {
        val memory = MemoryPreferences()
        val store = ProviderStore(memory.prefs, cipher)
        store.save(profile, fixture)
        memory.values["theme_mode"] = "Dark"
        val before = memory.values.toMap()
        memory.failNextCommit = true
        assertThrows(IllegalStateException::class.java) {
            store.save(profile.copy(mainModel = "fixture"), fixture.reversedArray())
        }
        assertEquals(before, memory.values)
        assertArrayEquals(fixture, store.loadApiKey())
    }

    @Test fun missingIvDoesNotClaimStoredKeyAndEmptyReplacementIsRejected() {
        val memory = MemoryPreferences()
        memory.values["key_blob"] = "incomplete"
        val store = ProviderStore(memory.prefs, cipher)
        assertFalse(store.hasApiKey)
        assertThrows(IllegalStateException::class.java) { store.save(profile) }
        assertThrows(IllegalArgumentException::class.java) { store.save(profile, byteArrayOf()) }
        assertEquals(0, memory.commits)
    }

    @Test fun firstSaveDiskFailureDoesNotClaimKeyWasSaved() {
        val memory = MemoryPreferences()
        val store = ProviderStore(memory.prefs, cipher)
        memory.failNextCommit = true
        assertThrows(IllegalStateException::class.java) { store.save(profile, fixture) }
        assertFalse(store.hasApiKey)
        assertFalse(store.configured)
        assertTrue(memory.values.isEmpty())
        assertNull(ProviderStore(memory.prefs, cipher).loadApiKey())
    }
}
