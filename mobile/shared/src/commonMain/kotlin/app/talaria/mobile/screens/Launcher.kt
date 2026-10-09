package app.talaria.mobile.screens

import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
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
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.unit.dp
import app.talaria.mobile.AppState
import app.talaria.mobile.Route
import app.talaria.mobile.Tags
import app.talaria.mobile.account.Account
import app.talaria.mobile.account.Instance
import app.talaria.mobile.instance.OriginResult
import app.talaria.mobile.instance.ProbeResult
import app.talaria.mobile.instance.normalizeOrigin
import app.talaria.mobile.mercury.Mercury
import kotlinx.coroutines.launch

const val TAG_NOTICE = "launcher-notice"

private sealed interface AddState {
    data object Idle : AddState

    data object Probing : AddState

    data class Rejected(val reason: String) : AddState
}

/**
 * The launcher: the instances this phone knows, the accounts signed in on each,
 * and the field that adds another.
 *
 * It is the desktop launcher's job on a phone, with one difference that matters:
 * accounts are listed *under* their instance, because an instance can hold more
 * than one signed-in person here. Desktop cannot do that — its isolation comes
 * from one cookie jar per webview, so one webview is one session.
 */
@Composable
fun LauncherScreen(state: AppState) {
    var input by remember { mutableStateOf("") }
    var add by remember { mutableStateOf<AddState>(AddState.Idle) }
    val scope = rememberCoroutineScope()

    fun probe() {
        when (val normalized = normalizeOrigin(input)) {
            is OriginResult.Invalid -> add = AddState.Rejected(normalized.reason)
            is OriginResult.Ok -> {
                add = AddState.Probing
                scope.launch {
                    add =
                        when (val probed = state.ports.probe(normalized.origin)) {
                            is ProbeResult.Failed -> AddState.Rejected(probed.reason)
                            is ProbeResult.Found -> {
                                state.addInstance(
                                    Instance.of(probed.origin, probed.beacon, probed.label),
                                )
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
            Text("Talaria", style = MaterialTheme.typography.headlineSmall, color = Mercury.Dark.readout)

            // Why the launcher is showing, when it is showing for a reason. An
            // account that vanished without explanation reads as a bug; the
            // commonest cause is the 7-day session TTL, which is a fact about
            // the api rather than a mistake anyone made.
            state.notice?.let { notice ->
                Text(
                    notice,
                    style = MaterialTheme.typography.bodySmall,
                    color = Mercury.Dark.warning,
                    modifier = Modifier.testTag(TAG_NOTICE),
                )
            }
            Text(
                "An instance is its origin. Add one to prove it answers.",
                style = MaterialTheme.typography.bodyMedium,
                color = Mercury.Dark.muted,
            )

            OutlinedTextField(
                value = input,
                onValueChange = {
                    input = it
                    if (add is AddState.Rejected) add = AddState.Idle
                },
                label = { Text("instance url") },
                placeholder = { Text("talaria.example.com") },
                singleLine = true,
                modifier = Modifier.fillMaxWidth().testTag(Tags.INSTANCE_URL),
            )

            Row(
                horizontalArrangement = Arrangement.spacedBy(12.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Button(
                    onClick = ::probe,
                    enabled = input.isNotBlank() && add !is AddState.Probing,
                    modifier = Modifier.testTag(Tags.ADD_INSTANCE),
                ) { Text("Add instance") }
                if (add is AddState.Probing) CircularProgressIndicator(modifier = Modifier.height(20.dp))
            }

            (add as? AddState.Rejected)?.let {
                // Failure is a sentence someone can act on; orange is a signal
                // here, never decoration.
                Text(
                    it.reason,
                    color = Mercury.Dark.danger,
                    style = MaterialTheme.typography.bodySmall,
                    modifier = Modifier.testTag(Tags.ADD_ERROR),
                )
            }

            if (state.instances.isEmpty()) {
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
                    items(state.instances, key = { it.id }) { instance ->
                        InstanceCard(state, instance)
                    }
                }
            }
        }
    }
}

@Composable
private fun InstanceCard(
    state: AppState,
    instance: Instance,
) {
    val signedIn = state.accountsOn(instance.id)
    Card(modifier = Modifier.fillMaxWidth().testTag(Tags.INSTANCE_ROW)) {
        Column(Modifier.padding(12.dp), verticalArrangement = Arrangement.spacedBy(6.dp)) {
            Text(instance.label, color = Mercury.Dark.readout, style = MaterialTheme.typography.titleSmall)
            Text(
                instance.origin,
                color = Mercury.Dark.muted,
                style = MaterialTheme.typography.bodySmall,
                fontFamily = FontFamily.Monospace,
            )

            signedIn.forEach { account -> AccountRow(state, account) }

            Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                TextButton(
                    onClick = { state.go(Route.SignIn(instance)) },
                    modifier = Modifier.testTag(Tags.SIGN_IN),
                ) { Text(if (signedIn.isEmpty()) "Sign in" else "Add another account") }
                TextButton(
                    onClick = { state.removeInstance(instance) },
                    modifier = Modifier.testTag(Tags.REMOVE_INSTANCE),
                ) { Text("Remove", color = Mercury.Dark.danger) }
            }
        }
    }
}

@Composable
private fun AccountRow(
    state: AppState,
    account: Account,
) {
    Row(
        modifier =
            Modifier
                .fillMaxWidth()
                .testTag(Tags.ACCOUNT_ROW)
                .clickable { state.go(Route.Signed(account)) }
                .padding(vertical = 4.dp),
        horizontalArrangement = Arrangement.SpaceBetween,
    ) {
        Text(account.shown, color = Mercury.Dark.readout, style = MaterialTheme.typography.bodyMedium)
        Text("open", color = Mercury.accent, style = MaterialTheme.typography.labelMedium)
    }
}
