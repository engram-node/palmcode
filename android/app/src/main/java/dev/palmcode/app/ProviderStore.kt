package dev.palmcode.app

import android.content.Context
import android.content.SharedPreferences
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import dev.palmcode.core.ProviderConfigRecord
import java.security.KeyStore
import java.util.Base64
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
class ProviderStore internal constructor(
    private val prefs: SharedPreferences,
    private val keyCipher: ApiKeyCipher,
) {
    constructor(context: Context) : this(context, "provider")
    internal constructor(context: Context, preferencesName: String) : this(
        context.getSharedPreferences(preferencesName, Context.MODE_PRIVATE),
        KeystoreApiKeyCipher,
    )

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

    var hasApiKey by mutableStateOf(
        !prefs.getString("key_blob", null).isNullOrEmpty() &&
            !prefs.getString("key_iv", null).isNullOrEmpty(),
    )
        private set

    /** Encrypt before editing preferences; acknowledge only a durable whole-profile save. */
    @Synchronized
    fun save(settings: ProviderSettings, replacementKey: ByteArray? = null) {
        require(replacementKey == null || replacementKey.isNotEmpty()) { "Empty replacement key" }
        check(replacementKey != null || hasApiKey) { "API key is required" }
        // A Keystore failure must not leave a newly saved URL with the old/missing key.
        val sealed = replacementKey?.let(keyCipher::encrypt)
        val values = linkedMapOf<String, Any?>(
            "base_url" to settings.baseUrl,
            "main_model" to settings.mainModel,
            "fast_model" to settings.fastModel,
            "context_window" to settings.contextWindow.toInt(),
            "max_output_tokens" to settings.maxOutputTokens.toInt(),
            "sampling_json" to settings.samplingJson,
            "extra_body_json" to settings.extraBodyJson,
            "continuous_usage" to settings.continuousUsage,
            "first_token_secs" to settings.firstTokenSecs.toLong(),
            "idle_secs" to settings.idleSecs.toLong(),
            "configured" to (settings.mainModel.isNotBlank() && settings.baseUrl.isNotBlank()),
        )
        if (sealed != null) {
            values["key_blob"] = Base64.getEncoder().encodeToString(sealed.blob)
            values["key_iv"] = Base64.getEncoder().encodeToString(sealed.iv)
        }
        val previous = prefs.all.filterKeys { it in values }
        try {
            check(prefs.edit().putValues(values).commit()) { "Provider storage write failed" }
        } catch (failure: Exception) {
            // SharedPreferences updates its memory cache even when its disk commit fails.
            // Restore just our keys, retaining appearance and unrelated preferences.
            runCatching {
                val rollback = prefs.edit()
                values.keys.forEach { rollback.remove(it) }
                rollback.putValues(previous).commit()
            }
            throw failure
        }
        hasApiKey = true
    }

    /**
     * Decrypt the key into the returned ByteArray. Caller must fill(0) it
     * after handing it to Rust — the String handed over is an unavoidable
     * JVM heap copy (Rust zeroizes its own buffers on drop).
     */
    @Synchronized
    fun loadApiKey(): ByteArray? {
        val blobB64 = prefs.getString("key_blob", null) ?: return null
        val ivB64 = prefs.getString("key_iv", null) ?: return null
        return keyCipher.decrypt(SealedApiKey(Base64.getDecoder().decode(blobB64), Base64.getDecoder().decode(ivB64)))
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

    private object KeystoreApiKeyCipher : ApiKeyCipher {
        private const val ALIAS = "palmcode_api_key"

        override fun encrypt(plain: ByteArray): SealedApiKey {
            val cipher = Cipher.getInstance("AES/GCM/NoPadding")
            cipher.init(Cipher.ENCRYPT_MODE, keystoreKey())
            return SealedApiKey(cipher.doFinal(plain), cipher.iv)
        }

        override fun decrypt(sealed: SealedApiKey): ByteArray {
            val cipher = Cipher.getInstance("AES/GCM/NoPadding")
            cipher.init(Cipher.DECRYPT_MODE, keystoreKey(), GCMParameterSpec(128, sealed.iv))
            return cipher.doFinal(sealed.blob)
        }

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

data class ProviderSettings(
    val baseUrl: String,
    val mainModel: String,
    val fastModel: String = "",
    val contextWindow: UInt = 128_000u,
    val maxOutputTokens: UInt = 16_384u,
    val samplingJson: String? = null,
    val extraBodyJson: String? = null,
    val continuousUsage: Boolean = false,
    val firstTokenSecs: ULong = 120UL,
    val idleSecs: ULong = 60UL,
)

internal data class SealedApiKey(val blob: ByteArray, val iv: ByteArray)
internal interface ApiKeyCipher {
    fun encrypt(plain: ByteArray): SealedApiKey
    fun decrypt(sealed: SealedApiKey): ByteArray
}

private fun SharedPreferences.Editor.putValues(values: Map<String, *>): SharedPreferences.Editor {
    values.forEach { (key, value) ->
        when (value) {
            null -> remove(key)
            is String -> putString(key, value)
            is Boolean -> putBoolean(key, value)
            is Int -> putInt(key, value)
            is Long -> putLong(key, value)
            else -> error("Unsupported provider preference type")
        }
    }
    return this
}
