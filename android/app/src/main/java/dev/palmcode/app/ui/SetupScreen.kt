package dev.palmcode.app.ui

import android.os.Build
import androidx.activity.compose.BackHandler
import androidx.compose.foundation.selection.selectable
import androidx.compose.foundation.selection.selectableGroup
import androidx.compose.foundation.selection.toggleable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.safeDrawingPadding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Button
import androidx.compose.material3.Checkbox
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.RadioButton
import androidx.compose.material3.Surface
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.input.PasswordVisualTransformation
import androidx.compose.ui.unit.dp
import dev.palmcode.app.CoreManager
import dev.palmcode.app.ThemeMode
import dev.palmcode.core.ProbeReportRecord
import dev.palmcode.core.providerProbe
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext

/** M1 provider setup: endpoint + BYOK key, probe, capability detection. */
@Composable
fun SetupScreen(onSaved: () -> Unit) {
    val store = CoreManager.store
    val scope = rememberCoroutineScope()
    BackHandler(enabled = store.configured, onBack = onSaved)

    var baseUrl by remember { mutableStateOf(store.baseUrl) }
    var apiKey by remember { mutableStateOf("") }
    var mainModel by remember { mutableStateOf(store.mainModel) }
    var fastModel by remember { mutableStateOf(store.fastModel) }

    var probing by remember { mutableStateOf(false) }
    var probe by remember { mutableStateOf<ProbeReportRecord?>(null) }
    var probeError by remember { mutableStateOf<String?>(null) }
    var advanced by remember { mutableStateOf(false) }

    // detected capabilities; user-overridable
    var continuousUsage by remember { mutableStateOf(store.continuousUsage) }
    var contextWindow by remember { mutableStateOf(store.contextWindow.toString()) }
    var maxOutputTokens by remember { mutableStateOf(store.maxOutputTokens.toString()) }
    var samplingJson by remember { mutableStateOf(store.samplingJson ?: "") }
    var extraBodyJson by remember { mutableStateOf(store.extraBodyJson ?: "") }
    var firstTokenSecs by remember { mutableStateOf(store.firstTokenSecs.toString()) }
    var idleSecs by remember { mutableStateOf(store.idleSecs.toString()) }

    val cleartext = baseUrl.startsWith("http://")

    Column(
        Modifier
            .fillMaxSize()
            .safeDrawingPadding()
            .verticalScroll(rememberScrollState())
            .padding(16.dp),
        verticalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        if (store.configured) {
            TextButton(onClick = onSaved) { Text("< Chats") }
        }
        Text("Settings", style = MaterialTheme.typography.headlineMedium)
        AppearanceSection()
        Text("Provider", style = MaterialTheme.typography.titleLarge)

        if (cleartext) {
            Surface(color = MaterialTheme.colorScheme.errorContainer, contentColor = MaterialTheme.colorScheme.onErrorContainer) {
                Text(
                    "http:// endpoint — credentials and traffic unencrypted. Only acceptable on LAN/Tailscale.",
                    Modifier.padding(8.dp),
                    style = MaterialTheme.typography.bodySmall,
                )
            }
        }

        OutlinedTextField(
            value = baseUrl, onValueChange = { baseUrl = it },
            label = { Text("Base URL (…/v1)") }, singleLine = true,
            modifier = Modifier.fillMaxWidth(),
        )
        OutlinedTextField(
            value = apiKey, onValueChange = { apiKey = it },
            label = { Text(if (store.hasApiKey) "API key (stored — leave blank to keep)" else "API key") },
            singleLine = true,
            visualTransformation = PasswordVisualTransformation(),
            keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Password),
            modifier = Modifier.fillMaxWidth(),
        )
        OutlinedTextField(
            value = mainModel, onValueChange = { mainModel = it },
            label = { Text("Main model") }, singleLine = true,
            modifier = Modifier.fillMaxWidth(),
        )
        OutlinedTextField(
            value = fastModel, onValueChange = { fastModel = it },
            label = { Text("Fast model (optional)") }, singleLine = true,
            modifier = Modifier.fillMaxWidth(),
        )

        Row(verticalAlignment = Alignment.CenterVertically) {
            Button(
                enabled = !probing && baseUrl.isNotBlank() && (apiKey.isNotBlank() || store.hasApiKey),
                onClick = {
                    // stage values so the probe runs against the edited config
                    store.baseUrl = baseUrl
                    store.mainModel = mainModel
                    store.fastModel = fastModel
                    store.continuousUsage = continuousUsage
                    store.firstTokenSecs = firstTokenSecs.toULongOrNull() ?: 120UL
                    store.idleSecs = idleSecs.toULongOrNull() ?: 60UL
                    if (apiKey.isNotBlank()) {
                        val bytes = apiKey.toByteArray(Charsets.UTF_8)
                        store.saveApiKey(bytes)
                        bytes.fill(0)
                    }
                    CoreManager.configureFromStore()
                    probing = true
                    probeError = null
                    scope.launch {
                        val r = withContext(Dispatchers.IO) {
                            runCatching { providerProbe() }
                        }
                        probing = false
                        r.onSuccess {
                            probe = it
                            // prefill capability defaults from the probe; user can override
                            continuousUsage = it.continuousUsage
                            it.maxModelLen?.let { len -> contextWindow = len.toString() }
                        }.onFailure { probeError = it.message }
                    }
                },
            ) { Text(if (probing) "Probing…" else "Test connection") }
            Spacer(Modifier.padding(4.dp))
            OutlinedButton(onClick = { advanced = !advanced }) {
                Text(if (advanced) "Hide advanced" else "Advanced")
            }
        }

        probeError?.let {
            Surface(color = MaterialTheme.colorScheme.errorContainer, contentColor = MaterialTheme.colorScheme.onErrorContainer) {
                Text(it, Modifier.padding(8.dp), style = MaterialTheme.typography.bodySmall)
            }
        }
        probe?.let { ProbePanel(it) }

        if (advanced) {
            Row(verticalAlignment = Alignment.CenterVertically) {
                Checkbox(checked = continuousUsage, onCheckedChange = { continuousUsage = it })
                Text("continuous_usage_stats", style = MaterialTheme.typography.bodyMedium)
            }
            OutlinedTextField(
                value = contextWindow, onValueChange = { contextWindow = it.filter(Char::isDigit) },
                label = { Text("Context window (tokens)") }, singleLine = true,
                keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Number),
                modifier = Modifier.fillMaxWidth(),
            )
            OutlinedTextField(
                value = maxOutputTokens, onValueChange = { maxOutputTokens = it.filter(Char::isDigit) },
                label = { Text("Max output tokens") }, singleLine = true,
                keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Number),
                modifier = Modifier.fillMaxWidth(),
            )
            OutlinedTextField(
                value = firstTokenSecs, onValueChange = { firstTokenSecs = it.filter(Char::isDigit) },
                label = { Text("First-token timeout (s)") }, singleLine = true,
                keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Number),
                modifier = Modifier.fillMaxWidth(),
            )
            OutlinedTextField(
                value = idleSecs, onValueChange = { idleSecs = it.filter(Char::isDigit) },
                label = { Text("Idle-stall timeout (s)") }, singleLine = true,
                keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Number),
                modifier = Modifier.fillMaxWidth(),
            )
            OutlinedTextField(
                value = samplingJson, onValueChange = { samplingJson = it },
                label = { Text("Sampling JSON (empty = server defaults)") },
                modifier = Modifier.fillMaxWidth(),
            )
            OutlinedTextField(
                value = extraBodyJson, onValueChange = { extraBodyJson = it },
                label = { Text("extra_body JSON (e.g. chat_template_kwargs)") },
                modifier = Modifier.fillMaxWidth(),
            )
        }

        Spacer(Modifier.height(8.dp))
        Button(
            enabled = baseUrl.isNotBlank() && mainModel.isNotBlank() && (apiKey.isNotBlank() || store.hasApiKey),
            onClick = {
                store.baseUrl = baseUrl
                store.mainModel = mainModel
                store.fastModel = fastModel
                store.continuousUsage = continuousUsage
                store.contextWindow = contextWindow.toUIntOrNull() ?: 128_000u
                store.maxOutputTokens = maxOutputTokens.toUIntOrNull() ?: 16_384u
                store.firstTokenSecs = firstTokenSecs.toULongOrNull() ?: 120UL
                store.idleSecs = idleSecs.toULongOrNull() ?: 60UL
                store.samplingJson = samplingJson.ifBlank { null }
                store.extraBodyJson = extraBodyJson.ifBlank { null }
                if (apiKey.isNotBlank()) {
                    val bytes = apiKey.toByteArray(Charsets.UTF_8)
                    store.saveApiKey(bytes)
                    bytes.fill(0)
                }
                store.configured = true
                if (CoreManager.configureFromStore()) onSaved()
            },
            modifier = Modifier.fillMaxWidth(),
        ) { Text("Save and continue") }
    }
}

@Composable
private fun AppearanceSection() {
    val store = CoreManager.store
    Text("Appearance", style = MaterialTheme.typography.titleLarge)
    Text("Theme", style = MaterialTheme.typography.titleMedium)
    Row(Modifier.fillMaxWidth().selectableGroup()) {
        ThemeMode.entries.forEach { mode ->
            Row(
                Modifier.weight(1f).selectable(
                    selected = store.themeMode == mode,
                    onClick = { store.setAppearance(mode = mode) },
                    role = Role.RadioButton,
                ).heightIn(min = 48.dp).padding(vertical = 8.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                RadioButton(selected = store.themeMode == mode, onClick = null)
                Text(mode.name, style = MaterialTheme.typography.bodyMedium)
            }
        }
    }
    if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
        Row(
            Modifier.fillMaxWidth().toggleable(
                value = store.dynamicColor,
                onValueChange = { store.setAppearance(dynamic = it) },
                role = Role.Switch,
            ).padding(vertical = 8.dp),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.SpaceBetween,
        ) {
            Text("Dynamic color", style = MaterialTheme.typography.bodyLarge)
            Switch(checked = store.dynamicColor, onCheckedChange = null)
        }
    }
}

@Composable
private fun ProbePanel(r: ProbeReportRecord) {
    Column(verticalArrangement = Arrangement.spacedBy(4.dp)) {
        Text("Probe", style = MaterialTheme.typography.titleMedium)
        Row("models", r.modelsOk, r.modelsDetail + if (r.modelIds.isNotEmpty()) " — ${r.modelIds.joinToString()}" else "")
        Row("max_model_len", r.maxModelLen != null, r.maxModelLen?.toString() ?: "not exposed")
        Row("tokenize", r.tokenizeOk, r.tokenizeDetail)
        Row("tool_choice", r.toolChoiceOk, r.toolChoiceDetail)
        Row("reasoning field", r.reasoningField != null, r.reasoningField ?: "none observed")
        Row("continuous usage", r.continuousUsage, if (r.continuousUsage) "stream_options accepted" else "final-chunk usage only")
        Row("cache counters", r.cachedTokens, if (r.cachedTokens) "cached_tokens exposed" else "not exposed")
        r.ttftMs?.let { Text("TTFT $it ms" + (r.sampleTps?.let { tps -> " · $tps tok/s" } ?: ""), style = MaterialTheme.typography.bodySmall) }
        r.chatError?.let {
            Surface(color = MaterialTheme.colorScheme.errorContainer, contentColor = MaterialTheme.colorScheme.onErrorContainer) {
                Text("sample chat failed: $it", Modifier.padding(8.dp), style = MaterialTheme.typography.bodySmall)
            }
        }
    }
}

@Composable
private fun Row(label: String, ok: Boolean, detail: String) {
    Row(verticalAlignment = Alignment.CenterVertically) {
        Text(
            if (ok) "OK " else "! ",
            color = if (ok) MaterialTheme.colorScheme.primary else MaterialTheme.colorScheme.error,
            style = MaterialTheme.typography.labelMedium,
        )
        Text("$label — $detail", style = MaterialTheme.typography.bodySmall)
    }
}
