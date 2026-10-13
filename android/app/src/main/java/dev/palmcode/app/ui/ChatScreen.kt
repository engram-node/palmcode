package dev.palmcode.app.ui

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.safeDrawingPadding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Button
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
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
import com.mikepenz.markdown.m3.Markdown
import dev.palmcode.app.AppState
import dev.palmcode.app.Approval
import dev.palmcode.app.ChatMsg
import dev.palmcode.app.CoreManager
import dev.palmcode.app.Screen
import dev.palmcode.app.ToolCard
import dev.palmcode.core.toolDiff
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext

@Composable
fun ChatScreen(sessionId: String) {
    val transcript = AppState.transcript(sessionId)
    androidx.activity.compose.BackHandler {
        CoreManager.refreshSessions()
        AppState.screen = Screen.Sessions
    }
    val listState = rememberLazyListState()
    var input by remember { mutableStateOf("") }
    val streaming = transcript.lastOrNull()?.streaming == true
    val context = androidx.compose.ui.platform.LocalContext.current

    // follow new turns and the streaming tail (only if near the bottom)
    LaunchedEffect(transcript.size) {
        if (transcript.isNotEmpty()) listState.animateScrollToItem(transcript.size - 1)
    }
    val tail = transcript.lastOrNull()
    LaunchedEffect(tail?.text?.length, tail?.reasoning?.length) {
        if (tail != null) {
            val lastVisible = listState.layoutInfo.visibleItemsInfo.lastOrNull()?.index ?: 0
            if (transcript.size - lastVisible <= 2) listState.scrollToItem(transcript.size - 1)
        }
    }
    // land at the message end (metrics footer) when a turn finishes
    LaunchedEffect(tail?.streaming) {
        if (tail != null && tail.streaming == false) listState.scrollToItem(transcript.size - 1)
    }

    Column(Modifier.fillMaxSize().safeDrawingPadding().imePadding()) {
        Row(
            Modifier.fillMaxWidth().padding(horizontal = 8.dp),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.SpaceBetween,
        ) {
            TextButton(onClick = {
                CoreManager.refreshSessions()
                AppState.screen = Screen.Sessions
            }) { Text("< Chats") }
            if (AppState.providerCleartext) {
                Text(
                    "http cleartext",
                    color = MaterialTheme.colorScheme.tertiary,
                    style = MaterialTheme.typography.labelSmall,
                )
            }
        }

        // workspace toggle: BUILD mode needs one linked root
        Row(Modifier.fillMaxWidth().padding(horizontal = 8.dp)) {
            val ws = AppState.workspace
            TextButton(onClick = { AppState.screen = Screen.Workspaces }) {
                Text(
                    ws?.substringAfterLast('/')?.let { "workspace: $it" } ?: "no workspace (chat only)",
                    style = MaterialTheme.typography.labelMedium,
                )
            }
        }

        LazyColumn(
            state = listState,
            modifier = Modifier.weight(1f).fillMaxWidth().padding(horizontal = 12.dp),
            verticalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            items(transcript, key = { it.id }) { msg ->
                MessageBubble(sessionId, msg)
            }
        }

        if (AppState.workspace != null) {
            val accepted = AppState.acceptEdits[sessionId] == true
            Row(Modifier.fillMaxWidth().padding(horizontal = 12.dp)) {
                TextButton(onClick = { CoreManager.setAcceptEdits(sessionId, !accepted) }) {
                    Text(
                        if (accepted) "edits: auto-accepted" else "edits: ask each time",
                        style = MaterialTheme.typography.labelMedium,
                        color = if (accepted) {
                            MaterialTheme.colorScheme.primary
                        } else {
                            MaterialTheme.colorScheme.outline
                        },
                    )
                }
            }
        }

        Row(
            Modifier.fillMaxWidth().padding(12.dp),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            OutlinedTextField(
                value = input,
                onValueChange = { input = it },
                placeholder = { Text("Message") },
                modifier = Modifier.weight(1f),
                maxLines = 4,
            )
            if (streaming) {
                Button(onClick = { CoreManager.stop(sessionId) }) { Text("Stop") }
            } else {
                Button(
                    enabled = input.isNotBlank(),
                    onClick = {
                        val text = input.trim()
                        input = ""
                        CoreManager.send(sessionId, text).onFailure {
                            android.util.Log.e("palmcode", "send failed: $it")
                            android.widget.Toast.makeText(
                                context, "Send failed: ${it.message}", android.widget.Toast.LENGTH_LONG,
                            ).show()
                        }
                    },
                ) { Text("Send") }
            }
        }
    }

    AppState.approvals.firstOrNull { it.sessionId == sessionId }?.let { approval ->
        ApprovalSheet(approval)
    }
}

@Composable
private fun MessageBubble(sessionId: String, msg: ChatMsg) {
    if (msg.role == "user") {
        Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.End) {
            Surface(
                color = MaterialTheme.colorScheme.primaryContainer,
                shape = RoundedCornerShape(12.dp),
            ) {
                Text(msg.text, Modifier.padding(10.dp), style = MaterialTheme.typography.bodyMedium)
            }
        }
        return
    }
    // assistant
    Column(Modifier.fillMaxWidth()) {
        if (msg.reasoning.isNotEmpty()) {
            ReasoningPart(msg)
        }
        when {
            msg.text.isNotEmpty() -> Markdown(
                content = msg.text,
                modifier = Modifier.fillMaxWidth().padding(vertical = 4.dp),
            )
            msg.streaming -> Text(
                "…",
                style = MaterialTheme.typography.bodyMedium,
                color = MaterialTheme.colorScheme.outline,
            )
        }
        if (msg.tools.isNotEmpty()) {
            Column(
                Modifier.fillMaxWidth().padding(vertical = 2.dp),
                verticalArrangement = Arrangement.spacedBy(4.dp),
            ) {
                for (card in msg.tools) ToolCardView(card)
            }
        }
        msg.error?.let { err ->
            Surface(color = MaterialTheme.colorScheme.errorContainer) {
                Column(Modifier.padding(8.dp)) {
                    Text(err, style = MaterialTheme.typography.bodySmall)
                    TextButton(onClick = { CoreManager.resumeChat(sessionId) }) { Text("Resume") }
                }
            }
        }
        if (msg.interrupted) {
            Row(verticalAlignment = Alignment.CenterVertically) {
                Text(
                    "interrupted",
                    color = MaterialTheme.colorScheme.tertiary,
                    style = MaterialTheme.typography.bodySmall,
                )
                TextButton(onClick = { CoreManager.resumeChat(sessionId) }) { Text("Resume") }
            }
        }
        if (!msg.streaming && msg.completionTokens != null) {
            MetricsLine(msg)
        }
    }
}

@Composable
private fun ReasoningPart(msg: ChatMsg) {
    // reasoning collapses once the answer starts; expandable for review
    var expanded by remember(msg.id) { mutableStateOf(msg.streaming) }
    Surface(
        color = MaterialTheme.colorScheme.surfaceVariant,
        shape = RoundedCornerShape(8.dp),
        modifier = Modifier.fillMaxWidth(),
    ) {
        Column(Modifier.padding(8.dp)) {
            TextButton(onClick = { expanded = !expanded }) {
                Text(
                    (if (expanded) "▾ reasoning" else "▸ reasoning") + " · ${msg.reasoning.length} chars",
                    style = MaterialTheme.typography.labelMedium,
                )
            }
            if (expanded) {
                Text(
                    msg.reasoning,
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
            }
        }
    }
}

@Composable
private fun MetricsLine(msg: ChatMsg) {
    val parts = buildList {
        msg.ttftMs?.let { add("ttft ${it} ms") }
        msg.decodeTps?.let { add("%.1f tok/s".format(it)) }
        msg.completionTokens?.let { add("$it tok") }
        msg.cachePct?.let { add("cache $it%") }
    }
    Text(
        parts.joinToString(" · "),
        style = MaterialTheme.typography.labelSmall,
        color = MaterialTheme.colorScheme.outline,
    )
}

@Composable
private fun ToolCardView(card: ToolCard) {
    var expanded by remember(card.id) { mutableStateOf(false) }
    var diffs by remember(card.id) { mutableStateOf<List<dev.palmcode.core.ToolDiffRecord>?>(null) }
    LaunchedEffect(card.id, expanded, card.done) {
        if (expanded && card.done && (card.name == "write" || card.name == "edit" || card.name == "apply_patch")) {
            diffs = withContext(Dispatchers.IO) { runCatching { toolDiff(card.id) }.getOrNull() }
        }
    }
    Surface(
        color = MaterialTheme.colorScheme.surfaceVariant,
        shape = RoundedCornerShape(8.dp),
        modifier = Modifier.fillMaxWidth(),
    ) {
        Column(Modifier.padding(6.dp)) {
            TextButton(onClick = { expanded = !expanded }) {
                val icon = when {
                    !card.done -> "…"
                    card.ok -> "✓"
                    else -> "✗"
                }
                val label = buildString {
                    append(if (expanded) "▾ " else "▸ ")
                    append("$icon ${card.name}")
                    card.path?.let { append(" $it") }
                    if (card.done) append(" · ${card.durationMs} ms")
                }
                Text(label, style = MaterialTheme.typography.labelMedium)
            }
            if (expanded) {
                if (card.preview.isNotEmpty()) {
                    Text(
                        card.preview,
                        style = MaterialTheme.typography.bodySmall,
                        fontFamily = androidx.compose.ui.text.font.FontFamily.Monospace,
                        modifier = Modifier.padding(horizontal = 8.dp),
                    )
                }
                diffs?.forEach { d ->
                    if (d.diff.isNotEmpty()) {
                        Text(
                            d.path,
                            style = MaterialTheme.typography.labelSmall,
                            modifier = Modifier.padding(horizontal = 8.dp, vertical = 2.dp),
                        )
                        for (line in d.diff.lineSequence()) {
                            val color = when {
                                line.startsWith("---") || line.startsWith("+++") || line.startsWith("@@") ->
                                    MaterialTheme.colorScheme.outline
                                line.startsWith("-") -> MaterialTheme.colorScheme.error
                                line.startsWith("+") -> MaterialTheme.colorScheme.primary
                                else -> MaterialTheme.colorScheme.onSurfaceVariant
                            }
                            Text(
                                line,
                                style = MaterialTheme.typography.bodySmall,
                                fontFamily = androidx.compose.ui.text.font.FontFamily.Monospace,
                                color = color,
                                modifier = Modifier.padding(horizontal = 8.dp),
                            )
                        }
                    }
                }
            }
        }
    }
}

@Composable
private fun ApprovalSheet(approval: Approval) {
    androidx.compose.material3.AlertDialog(
        onDismissRequest = { CoreManager.resolveApproval(approval.requestId, false, "once") },
        title = { Text("Allow ${approval.tool}?") },
        text = {
            Column {
                Text(approval.path, style = MaterialTheme.typography.bodyMedium)
                if (approval.detail.isNotEmpty()) {
                    Text(
                        approval.detail,
                        style = MaterialTheme.typography.bodySmall,
                        fontFamily = androidx.compose.ui.text.font.FontFamily.Monospace,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                        modifier = Modifier.padding(top = 8.dp),
                    )
                }
                // extra scope choices beyond the default buttons
                Row(Modifier.padding(top = 8.dp)) {
                    TextButton(onClick = {
                        CoreManager.resolveApproval(approval.requestId, true, "session")
                    }) { Text("Allow for session") }
                    TextButton(onClick = {
                        CoreManager.resolveApproval(approval.requestId, true, "project")
                    }) { Text("Always in project") }
                }
            }
        },
        confirmButton = {
            TextButton(onClick = {
                CoreManager.resolveApproval(approval.requestId, true, "once")
            }) { Text("Allow once") }
        },
        dismissButton = {
            TextButton(onClick = {
                CoreManager.resolveApproval(approval.requestId, false, "once")
            }) { Text("Deny") }
        },
    )
}
