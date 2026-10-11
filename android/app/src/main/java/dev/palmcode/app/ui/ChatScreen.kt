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
import dev.palmcode.app.ChatMsg
import dev.palmcode.app.CoreManager
import dev.palmcode.app.Screen

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

        LazyColumn(
            state = listState,
            modifier = Modifier.weight(1f).fillMaxWidth().padding(horizontal = 12.dp),
            verticalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            items(transcript, key = { it.id }) { msg ->
                MessageBubble(sessionId, msg)
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
