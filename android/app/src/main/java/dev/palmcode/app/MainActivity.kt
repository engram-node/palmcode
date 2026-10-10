package dev.palmcode.app

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
import androidx.compose.material3.OutlinedButton
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
import dev.palmcode.app.ui.theme.PalmcodeTheme
import dev.palmcode.core.palmcodeCoreVersion
import dev.palmcode.core.palmcodeGreet

class MainActivity : ComponentActivity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        setContent {
            PalmcodeTheme {
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
        Text("palmcode", style = MaterialTheme.typography.headlineLarge)
        Spacer(Modifier.height(8.dp))
        Text(palmcodeGreet("android"), style = MaterialTheme.typography.bodyLarge)
        Text("core ${palmcodeCoreVersion()}", style = MaterialTheme.typography.bodySmall)
        Spacer(Modifier.height(24.dp))
        Button(onClick = { probeResult = runExecProbe(appContext) }) {
            Text("Run exec probe")
        }
        Spacer(Modifier.height(8.dp))
        Text(probeResult, style = MaterialTheme.typography.bodyMedium)
        Spacer(Modifier.height(24.dp))
        LicenseSection()
    }
}

// Apache-2.0 / GPL-2.0 notice obligations: assets are bundled by
// copyLicenseAssets (see build.gradle.kts); texts live at the repo root.
@Composable
fun LicenseSection() {
    val context = LocalContext.current
    var show by remember { mutableStateOf(false) }
    val text by remember {
        mutableStateOf(
            context.assets.open("THIRD_PARTY_LICENSES").bufferedReader().use { it.readText() },
        )
    }
    OutlinedButton(onClick = { show = !show }) {
        Text(if (show) "Hide third-party licenses" else "Third-party licenses")
    }
    if (show) {
        Spacer(Modifier.height(8.dp))
        Text(text, style = MaterialTheme.typography.bodySmall)
    }
}
