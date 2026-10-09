package app.talaria.mobile

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material3.Button
import androidx.compose.material3.Card
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateListOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.unit.dp
import app.talaria.mobile.instance.OriginResult
import app.talaria.mobile.instance.ProbeResult
import app.talaria.mobile.instance.normalizeOrigin
import app.talaria.mobile.instance.probeInstance
import app.talaria.mobile.mercury.Mercury
import app.talaria.mobile.mercury.MercuryTheme
import app.talaria.mobile.net.talariaHttpClient
import io.ktor.client.HttpClient
import kotlinx.coroutines.launch

/**
 * Phase 1 of docs/MOBILE.md: the shell and identity. An instance is added by
 * URL and proved by its beacon, which is the same contract the desktop launcher
 * holds — so this screen is the smallest thing that is genuinely the app rather
 * than a placeholder.
 *
 * What it is NOT yet: the account store (credentials per instance), and
 * therefore anything behind a session. That needs the device-credential class
 * the doc calls gap 1, and building a login against the 7-day cookie first
 * would mean building it twice.
 */
/** Stable handles for the headless UI driver. Behaviour tests address these
 *  rather than visible copy, so rewording a label is not a test failure. */
object Tags {
    const val INSTANCE_URL = "instance-url"
    const val ADD_INSTANCE = "add-instance"
    const val ADD_ERROR = "add-error"
    const val INSTANCE_LIST = "instance-list"
    const val EMPTY = "instances-empty"
    const val INSTANCE_ROW = "instance-row"
}

private sealed interface AddState {
    data object Idle : AddState

    data object Probing : AddState

    data class Rejected(val reason: String) : AddState
}

@Composable
fun App(client: HttpClient = remember { talariaHttpClient() }) {
    MercuryTheme {
        val instances = remember { mutableStateListOf<ProbeResult.Found>() }
        var input by remember { mutableStateOf("") }
        var state by remember { mutableStateOf<AddState>(AddState.Idle) }
        val scope = rememberCoroutineScope()

        fun add() {
            when (val normalized = normalizeOrigin(input)) {
                is OriginResult.Invalid -> state = AddState.Rejected(normalized.reason)
                is OriginResult.Ok -> {
                    state = AddState.Probing
                    scope.launch {
                        state =
                            when (val probed = probeInstance(client, normalized.origin)) {
                                is ProbeResult.Failed -> AddState.Rejected(probed.reason)
                                is ProbeResult.Found -> {
                                    // The beacon uuid is the dedupe key, not the
                                    // URL: one instance reached by two names is
                                    // one instance.
                                    val known = instances.any { it.beacon.instance == probed.beacon.instance }
                                    if (!known) instances.add(probed)
                                    input = ""
                                    AddState.Idle
                                }
                            }
                    }
                }
            }
        }

        Scaffold(containerColor = Mercury.Dark.ground) { padding ->
            Column(
                modifier = Modifier.fillMaxSize().padding(padding).padding(16.dp),
                verticalArrangement = Arrangement.spacedBy(12.dp),
            ) {
                Text(
                    "Talaria",
                    style = MaterialTheme.typography.headlineSmall,
                    color = Mercury.Dark.readout,
                )
                Text(
                    "An instance is its origin. Add one to prove it answers.",
                    style = MaterialTheme.typography.bodyMedium,
                    color = Mercury.Dark.muted,
                )

                OutlinedTextField(
                    value = input,
                    onValueChange = {
                        input = it
                        if (state is AddState.Rejected) state = AddState.Idle
                    },
                    label = { Text("instance url") },
                    placeholder = { Text("talaria.example.com") },
                    singleLine = true,
                    // Test tags, not text, are what the headless driver finds:
                    // a label someone rewords should not break a test about
                    // behaviour (mobile/shared/src/commonTest/.../UiHarness.kt).
                    modifier = Modifier.fillMaxWidth().testTag(Tags.INSTANCE_URL),
                )

                Row(
                    horizontalArrangement = Arrangement.spacedBy(12.dp),
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    Button(
                        onClick = ::add,
                        enabled = input.isNotBlank() && state !is AddState.Probing,
                        modifier = Modifier.testTag(Tags.ADD_INSTANCE),
                    ) { Text("Add instance") }
                    if (state is AddState.Probing) {
                        CircularProgressIndicator(modifier = Modifier.height(20.dp))
                    }
                }

                (state as? AddState.Rejected)?.let {
                    // Failure is a sentence someone can act on — orange as an
                    // outline-and-label signal, never a decorative fill.
                    Text(
                        it.reason,
                        color = Mercury.Dark.danger,
                        style = MaterialTheme.typography.bodySmall,
                        modifier = Modifier.testTag(Tags.ADD_ERROR),
                    )
                }

                Spacer(Modifier.height(4.dp))

                if (instances.isEmpty()) {
                    Text(
                        "No instances yet.",
                        color = Mercury.Dark.inkDim,
                        style = MaterialTheme.typography.bodySmall,
                        modifier = Modifier.testTag(Tags.EMPTY),
                    )
                } else {
                    LazyColumn(
                        verticalArrangement = Arrangement.spacedBy(8.dp),
                        modifier = Modifier.testTag(Tags.INSTANCE_LIST),
                    ) {
                        items(instances) { found -> InstanceRow(found) }
                    }
                }
            }
        }
    }
}

@Composable
private fun InstanceRow(found: ProbeResult.Found) {
    Card(modifier = Modifier.fillMaxWidth().testTag(Tags.INSTANCE_ROW)) {
        Column(Modifier.padding(12.dp), verticalArrangement = Arrangement.spacedBy(2.dp)) {
            Text(found.label, color = Mercury.Dark.readout, style = MaterialTheme.typography.titleSmall)
            Text(
                found.origin,
                color = Mercury.Dark.muted,
                style = MaterialTheme.typography.bodySmall,
                fontFamily = FontFamily.Monospace,
            )
            Text(
                found.beacon.instance,
                color = Mercury.Dark.inkDim,
                style = MaterialTheme.typography.labelSmall,
                fontFamily = FontFamily.Monospace,
            )
        }
    }
}
