package dev.palmcode.app.ui

import android.net.Uri
import androidx.activity.compose.BackHandler
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.safeDrawingPadding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateListOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.unit.dp
import dev.palmcode.app.AppState
import dev.palmcode.app.CoreManager
import dev.palmcode.app.Screen
import dev.palmcode.app.WorkspaceIO
import dev.palmcode.core.WorkspaceRecord
import dev.palmcode.core.workspaceImportDir
import dev.palmcode.core.workspaceMetrics
import dev.palmcode.core.workspaceRegister
import dev.palmcode.core.workspaceRemove
import dev.palmcode.core.workspaceSetTrusted
import dev.palmcode.core.workspacesList
import java.io.File
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext

private fun humanSize(bytes: ULong): String {
    var v = bytes.toDouble()
    val units = arrayOf("B", "KiB", "MiB", "GiB")
    var u = 0
    while (v >= 1024 && u < units.lastIndex) {
        v /= 1024; u++
    }
    return "%.1f %s".format(v, units[u])
}

/** M2 E: workspace registry — SAF import/export, dev path add, set/forget/delete. */
@Composable
fun WorkspaceScreen() {
    val context = LocalContext.current
    BackHandler { AppState.screen = Screen.Sessions }
    val rows = remember { mutableStateListOf<WorkspaceRecord>() }
    var busy by remember { mutableStateOf<String?>(null) }
    var showAddPath by remember { mutableStateOf(false) }
    var importUri by remember { mutableStateOf<Uri?>(null) }
    var exportUri by remember { mutableStateOf<Uri?>(null) }
    var refreshTick by remember { mutableStateOf(0) }
    val scope = rememberCoroutineScope()

    suspend fun refreshSilently() {
        val list = withContext(Dispatchers.IO) { workspacesList() }
        rows.clear()
        rows.addAll(list)
    }
    LaunchedEffect(refreshTick) { refreshSilently() }

    val importLauncher = rememberLauncherForActivityResult(
        ActivityResultContracts.OpenDocumentTree(),
    ) { uri: Uri? -> importUri = uri }

    val exportLauncher = rememberLauncherForActivityResult(
        ActivityResultContracts.OpenDocumentTree(),
    ) { uri: Uri? -> exportUri = uri }

    LaunchedEffect(importUri) {
        val uri = importUri ?: return@LaunchedEffect
        busy = "importing…"
        runCatching {
            withContext(Dispatchers.IO) {
                val name = WorkspaceIO.treeName(context, uri) ?: "import"
                var dest = File(workspaceImportDir(), name)
                var n = 2
                while (dest.exists()) dest = File(workspaceImportDir(), "$name-${n++}")
                val files = WorkspaceIO.importTree(context, uri, dest)
                workspaceRegister(dest.name, dest.absolutePath)
                files
            }
        }.onFailure { android.util.Log.e("palmcode", "import failed: $it") }
        busy = null
        importUri = null
        refreshTick++
    }

    LaunchedEffect(exportUri) {
        val uri = exportUri ?: return@LaunchedEffect
        val ws = AppState.workspace ?: return@LaunchedEffect
        busy = "exporting…"
        runCatching {
            withContext(Dispatchers.IO) { WorkspaceIO.exportTree(context, File(ws), uri) }
        }.onFailure { android.util.Log.e("palmcode", "export failed: $it") }
        busy = null
        exportUri = null
    }

    Column(Modifier.fillMaxSize().safeDrawingPadding().padding(12.dp)) {
        Row(verticalAlignment = Alignment.CenterVertically) {
            TextButton(onClick = { AppState.screen = Screen.Sessions }) { Text("<") }
            Text("Workspaces", style = MaterialTheme.typography.titleMedium)
        }
        Text(
            busy ?: (AppState.workspace?.let { "active: $it" } ?: "no workspace set"),
            style = MaterialTheme.typography.labelSmall,
            color = MaterialTheme.colorScheme.outline,
            modifier = Modifier.padding(vertical = 4.dp),
        )
        Row(Modifier.fillMaxWidth().padding(vertical = 4.dp)) {
            TextButton(onClick = { importLauncher.launch(null) }) { Text("Import (SAF)") }
            TextButton(onClick = { showAddPath = true }) { Text("Add path") }
            if (AppState.workspace != null) {
                TextButton(onClick = { exportLauncher.launch(null) }) { Text("Export") }
                TextButton(onClick = { CoreManager.clearWorkspace() }) { Text("Chat only") }
            }
        }
        LazyColumn(Modifier.weight(1f).fillMaxWidth()) {
            items(rows, key = { it.id }) { w ->
                WorkspaceRow(
                    w,
                    active = AppState.workspace == w.rootPath,
                    onOpen = {
                        CoreManager.setWorkspace(w.rootPath)
                        AppState.screen = Screen.Sessions
                    },
                    onForget = {
                        workspaceRemove(w.id, false)
                        refreshTick++
                    },
                    onDelete = {
                        workspaceRemove(w.id, true)
                        refreshTick++
                    },
                    onToggleTrust = {
                        scope.launch(Dispatchers.IO) {
                            runCatching { workspaceSetTrusted(w.id, !w.trusted) }
                            withContext(Dispatchers.Main) { refreshTick++ }
                        }
                    },
                )
            }
        }
    }

    if (showAddPath) {
        var path by remember { mutableStateOf("") }
        var err by remember { mutableStateOf<String?>(null) }
        AlertDialog(
            onDismissRequest = { showAddPath = false },
            title = { Text("Add workspace path") },
            text = {
                Column {
                    OutlinedTextField(
                        value = path,
                        onValueChange = { path = it },
                        placeholder = { Text("/storage/emulated/0/src/my-project") },
                        singleLine = true,
                    )
                    err?.let { Text(it, color = MaterialTheme.colorScheme.error) }
                }
            },
            confirmButton = {
                TextButton(onClick = {
                    runCatching {
                        workspaceRegister(File(path).name, path)
                        CoreManager.setWorkspace(path)
                    }.onSuccess {
                        showAddPath = false
                        err = null
                        AppState.screen = Screen.Sessions
                    }.onFailure { err = it.message }
                }) { Text("Use") }
            },
            dismissButton = { TextButton(onClick = { showAddPath = false }) { Text("Cancel") } },
        )
    }
}

@Composable
private fun WorkspaceRow(
    w: WorkspaceRecord,
    active: Boolean,
    onOpen: () -> Unit,
    onForget: () -> Unit,
    onDelete: () -> Unit,
    onToggleTrust: () -> Unit,
) {
    var metrics by remember(w.id) { mutableStateOf<String?>(null) }
    LaunchedEffect(w.id) {
        val m = withContext(Dispatchers.IO) { runCatching { workspaceMetrics(w.rootPath) }.getOrNull() }
        metrics = m?.let {
            if (it.present) "${it.fileCount} files · ${humanSize(it.sizeBytes)}" else "missing"
        }
    }
    Surface(
        color = if (active) MaterialTheme.colorScheme.primaryContainer
        else MaterialTheme.colorScheme.surface,
        shape = RoundedCornerShape(10.dp),
        modifier = Modifier.fillMaxWidth().padding(vertical = 4.dp),
    ) {
        Column(Modifier.padding(10.dp)) {
            Row(Modifier.fillMaxWidth().clickable(onClick = onOpen)) {
                Column(Modifier.weight(1f)) {
                    Text(w.name, style = MaterialTheme.typography.bodyMedium)
                    Text(
                        "${w.rootPath}\n${metrics ?: "…"}",
                        style = MaterialTheme.typography.labelSmall,
                        color = MaterialTheme.colorScheme.outline,
                    )
                }
            }
            Row {
                TextButton(onClick = onOpen) { Text(if (active) "Active" else "Open") }
                TextButton(onClick = onForget) { Text("Forget") }
                TextButton(onClick = onDelete) { Text("Delete copy") }
            }
            Row {
                // M3 trust: untrusted ignores the repo's .palmcode/config.json;
                // trusted honors its ask/deny rules (allows are always stripped)
                val label = if (w.trusted) "trusted ✓ — project ask/deny rules on"
                else "untrusted — project config ignored"
                TextButton(onClick = onToggleTrust) {
                    Text(label, color = if (w.trusted) MaterialTheme.colorScheme.primary
                    else MaterialTheme.colorScheme.outline)
                }
            }
        }
    }
}
