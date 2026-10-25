package dev.palmcode.app

import android.content.Context
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import android.util.Base64
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import dev.palmcode.core.ProviderConfigRecord
import java.security.KeyStore
import javax.crypto.Cipher
import javax.crypto.KeyGenerator
import javax.crypto.spec.GCMParameterSpec

enum class ThemeMode { System, Light, Dark }

/**
 * BYOK provider configuration. The API key lives only as a Keystore-wrapped
 * AES-GCM blob in SharedPreferences; decryption hands the plaintext straight
 * to Rust for in-memory use. The key is never written to disk in the clear
 * and never logged (no Log.* usage in this file, by design).
 */
class ProviderStore(context: Context) {

    private val prefs = context.getSharedPreferences("provider", Context.MODE_PRIVATE)

    // Load appearance once. Snapshot state updates the root theme immediately;
    // streamed content never reads preferences.
    var themeMode by mutableStateOf(
        prefs.getString("theme_mode", null).let { saved ->
            ThemeMode.entries.firstOrNull { it.name == saved } ?: ThemeMode.System
        },
    )
        private set
    var dynamicColor by mutableStateOf(prefs.getBoolean("dynamic_color", true))
        private set

    fun setAppearance(mode: ThemeMode = themeMode, dynamic: Boolean = dynamicColor) {
        prefs.edit().putString("theme_mode", mode.name).putBoolean("dynamic_color", dynamic).apply()
        themeMode = mode
        dynamicColor = dynamic
    }

    var baseUrl: String
        get() = prefs.getString("base_url", "") ?: ""
        set(v) = prefs.edit().putString("base_url", v).apply()
    var mainModel: String
        get() = prefs.getString("main_model", "") ?: ""
        set(v) = prefs.edit().putString("main_model", v).apply()
    var fastModel: String
        get() = prefs.getString("fast_model", "") ?: ""
        set(v) = prefs.edit().putString("fast_model", v).apply()
    var contextWindow: UInt
        get() = prefs.getInt("context_window", 128_000).toUInt()
        set(v) = prefs.edit().putInt("context_window", v.toInt()).apply()
    var maxOutputTokens: UInt
        get() = prefs.getInt("max_output_tokens", 16_384).toUInt()
        set(v) = prefs.edit().putInt("max_output_tokens", v.toInt()).apply()
    var samplingJson: String?
        get() = prefs.getString("sampling_json", null)
        set(v) = prefs.edit().putString("sampling_json", v).apply()
    var extraBodyJson: String?
        get() = prefs.getString("extra_body_json", null)
        set(v) = prefs.edit().putString("extra_body_json", v).apply()
    var continuousUsage: Boolean
        get() = prefs.getBoolean("continuous_usage", false)
        set(v) = prefs.edit().putBoolean("continuous_usage", v).apply()
    var firstTokenSecs: ULong
        get() = prefs.getLong("first_token_secs", 120L).toULong()
        set(v) = prefs.edit().putLong("first_token_secs", v.toLong()).apply()
    var idleSecs: ULong
        get() = prefs.getLong("idle_secs", 60L).toULong()
        set(v) = prefs.edit().putLong("idle_secs", v.toLong()).apply()
    var configured: Boolean
        get() = prefs.getBoolean("configured", false)
        set(v) = prefs.edit().putBoolean("configured", v).apply()

    val hasApiKey: Boolean get() = prefs.contains("key_blob")

    /** Keystore-encrypt the key and store only the ciphertext. */
    fun saveApiKey(plain: ByteArray) {
        val cipher = Cipher.getInstance("AES/GCM/NoPadding")
        cipher.init(Cipher.ENCRYPT_MODE, keystoreKey())
        val blob = cipher.doFinal(plain)
        prefs.edit()
            .putString("key_blob", Base64.encodeToString(blob, Base64.NO_WRAP))
            .putString("key_iv", Base64.encodeToString(cipher.iv, Base64.NO_WRAP))
            .apply()
    }

    /**
     * Decrypt the key into the returned ByteArray. Caller must fill(0) it
     * after handing it to Rust — the String handed over is an unavoidable
     * JVM heap copy (Rust zeroizes its own buffers on drop).
     */
    fun loadApiKey(): ByteArray? {
        val blobB64 = prefs.getString("key_blob", null) ?: return null
        val ivB64 = prefs.getString("key_iv", null) ?: return null
        val cipher = Cipher.getInstance("AES/GCM/NoPadding")
        val iv = Base64.decode(ivB64, Base64.NO_WRAP)
        cipher.init(Cipher.DECRYPT_MODE, keystoreKey(), GCMParameterSpec(128, iv))
        return cipher.doFinal(Base64.decode(blobB64, Base64.NO_WRAP))
    }

    /** Build the FFI record, decrypting the key inline. */
    fun toRecord(apiKey: String): ProviderConfigRecord = ProviderConfigRecord(
        baseUrl = baseUrl.trimEnd('/'),
        apiKey = apiKey,
        mainModel = mainModel,
        fastModel = fastModel,
        contextWindow = contextWindow,
        maxOutputTokens = maxOutputTokens,
        samplingJson = samplingJson?.ifBlank { null },
        extraBodyJson = extraBodyJson?.ifBlank { null },
        continuousUsage = continuousUsage,
        firstTokenSecs = firstTokenSecs,
        idleSecs = idleSecs,
    )

    val cleartext: Boolean get() = baseUrl.startsWith("http://")

    companion object {
        private const val ALIAS = "palmcode_api_key"

        private fun keystoreKey(): javax.crypto.SecretKey {
            val ks = KeyStore.getInstance("AndroidKeyStore").apply { load(null) }
            (ks.getEntry(ALIAS, null) as? KeyStore.SecretKeyEntry)?.let { return it.secretKey }
            val gen = KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, "AndroidKeyStore")
            gen.init(
                KeyGenParameterSpec.Builder(
                    ALIAS,
                    KeyProperties.PURPOSE_ENCRYPT or KeyProperties.PURPOSE_DECRYPT,
                )
                    .setBlockModes(KeyProperties.BLOCK_MODE_GCM)
                    .setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE)
                    .setKeySize(256)
                    .build(),
            )
            return gen.generateKey()
        }
    }
}
