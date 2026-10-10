package dev.pocket.app

import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Button
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.unit.dp
import dev.pocket.app.ui.theme.PocketTheme
import dev.pocket.core.pocketCoreVersion
import dev.pocket.core.pocketGreet

class MainActivity : ComponentActivity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        setContent {
            PocketTheme {
                Surface(modifier = Modifier.fillMaxSize()) {
                    HomeScreen()
                }
            }
        }
    }
}

@Composable
fun HomeScreen() {
    val appContext = LocalContext.current.applicationContext
    var probeResult by remember { mutableStateOf("exec probe: not run") }

    Column(
        Modifier
            .fillMaxSize()
            .verticalScroll(rememberScrollState())
            .padding(16.dp),
    ) {
        Text("pocket", style = MaterialTheme.typography.headlineLarge)
        Spacer(Modifier.height(8.dp))
        Text(pocketGreet("android"), style = MaterialTheme.typography.bodyLarge)
        Text("core ${pocketCoreVersion()}", style = MaterialTheme.typography.bodySmall)
        Spacer(Modifier.height(24.dp))
        Button(onClick = { probeResult = runExecProbe(appContext) }) {
            Text("Run exec probe")
        }
        Spacer(Modifier.height(8.dp))
        Text(probeResult, style = MaterialTheme.typography.bodyMedium)
    }
}
