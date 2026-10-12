package dev.palmcode.app.ui

import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.safeDrawingPadding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material3.Button
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import dev.palmcode.app.AppState
import dev.palmcode.app.CoreManager
import dev.palmcode.app.Screen
import java.text.SimpleDateFormat
import java.util.Date
import java.util.Locale

// Apache-2.0 / GPL-2.0 notice obligations: assets are bundled by
// copyLicenseAssets (see build.gradle.kts); texts live at the repo root.
@Composable
fun LicensesButton() {
    val context = androidx.compose.ui.platform.LocalContext.current
    var show by remember { mutableStateOf(false) }
    val text = remember {
        context.assets.open("THIRD_PARTY_LICENSES").bufferedReader().use { it.readText() }
    }
    TextButton(onClick = { show = true }) { Text("Licenses") }
    if (show) {
        androidx.compose.material3.AlertDialog(
            onDismissRequest = { show = false },
            confirmButton = { TextButton(onClick = { show = false }) { Text("Close") } },
            text = {
                androidx.compose.foundation.text.selection.SelectionContainer {
                    Text(text, style = MaterialTheme.typography.bodySmall)
                }
            },
        )
    }
}

@Composable
fun SessionListScreen() {
    LaunchedEffect(Unit) { CoreManager.refreshSessions() }

    Column(Modifier.fillMaxSize().safeDrawingPadding().padding(16.dp)) {
        Row(
            Modifier.fillMaxWidth(),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.SpaceBetween,
        ) {
            Text("palmcode", style = MaterialTheme.typography.headlineMedium)
            Row {
                LicensesButton()
                TextButton(onClick = { AppState.screen = Screen.Workspaces }) { Text("Workspaces") }
                TextButton(onClick = { AppState.screen = Screen.Setup }) { Text("Provider") }
            }
        }

        if (AppState.providerCleartext) {
            Surface(color = MaterialTheme.colorScheme.tertiaryContainer) {
                Text(
                    "cleartext http — LAN/Tailscale only",
                    Modifier.padding(8.dp).fillMaxWidth(),
                    style = MaterialTheme.typography.bodySmall,
                )
            }
        }

        Button(
            onClick = { CoreManager.openChat(CoreManager.newChat()) },
            modifier = Modifier.fillMaxWidth().padding(vertical = 8.dp),
        ) { Text("New chat") }

        LazyColumn(verticalArrangement = Arrangement.spacedBy(8.dp)) {
            items(AppState.sessions, key = { it.id }) { s ->
                Surface(
                    color = MaterialTheme.colorScheme.surfaceVariant,
                    modifier = Modifier
                        .fillMaxWidth()
                        .clickable { CoreManager.openChat(s.id) },
                ) {
                    Column(Modifier.padding(12.dp)) {
                        Text(s.title, style = MaterialTheme.typography.titleSmall)
                        Text(
                            "${s.status} · ${s.model} · " + SimpleDateFormat(
                                "MM-dd HH:mm", Locale.US,
                            ).format(Date(s.createdAtMs)),
                            style = MaterialTheme.typography.bodySmall,
                        )
                    }
                }
            }
        }
    }
}
