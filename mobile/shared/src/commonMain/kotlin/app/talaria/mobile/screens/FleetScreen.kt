package app.talaria.mobile.screens

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.material3.Card
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.unit.dp
import app.talaria.mobile.AppState
import app.talaria.mobile.account.Account
import app.talaria.mobile.account.Credential
import app.talaria.mobile.account.Instance
import app.talaria.mobile.mercury.Mercury
import app.talaria.mobile.net.AgentAction
import app.talaria.mobile.net.CommsResult
import app.talaria.mobile.net.CostResult
import app.talaria.mobile.net.FleetAgent
import kotlinx.coroutines.launch

/**
 * The fleet: who is up, who is not, and the switch.
 *
 * This is the surface that earns the word "controller". An agent misbehaving
 * at 11pm needs to be stoppable from wherever its manager is, and every other
 * way to do that is at a desk.
 *
 * Three actions, not the api's seven. `up`, `stop` and `restart` are
 * reversible in one tap and fit in a sentence — the scope line's own test.
 * `retire`, `unretire`, `delete` and `roll` change what the organisation *has*
 * rather than what it is *doing*; those want a desk and a diff.
 */
const val TAG_FLEET = "fleet"
const val TAG_FLEET_ROW = "fleet-row"
const val TAG_FLEET_DOT = "fleet-dot"
const val TAG_FLEET_EMPTY = "fleet-empty"
const val TAG_FLEET_ERROR = "fleet-error"
const val TAG_FLEET_UNHEALTHY = "fleet-unhealthy"
const val TAG_FLEET_STOP = "fleet-stop"
const val TAG_FLEET_START = "fleet-start"
const val TAG_FLEET_RESTART = "fleet-restart"
const val TAG_FLEET_UNMANAGED = "fleet-unmanaged"
const val TAG_COST = "fleet-cost"
const val TAG_COST_FORBIDDEN = "fleet-cost-forbidden"

private sealed interface FleetLoad {
    data object Busy : FleetLoad

    data class Ready(val agents: List<FleetAgent>) : FleetLoad

    data class Failed(val reason: String) : FleetLoad
}

@Composable
fun FleetScreen(
    state: AppState,
    account: Account,
    instance: Instance,
    credential: Credential,
) {
    var load by remember { mutableStateOf<FleetLoad>(FleetLoad.Busy) }
    var acting by remember { mutableStateOf<String?>(null) }
    var error by remember { mutableStateOf<String?>(null) }
    var cost by remember { mutableStateOf<CostResult?>(null) }
    val scope = rememberCoroutineScope()

    suspend fun load() {
        load =
            when (val result = state.ports.fleet(instance, credential)) {
                is CommsResult.Ok ->
                    // Trouble first: unhealthy, then stopped, then the rest.
                    // A roster sorted by name makes you hunt for the one thing
                    // that needs you, which is the opposite of a glance.
                    FleetLoad.Ready(
                        result.value.sortedWith(
                            compareByDescending<FleetAgent> { it.isUnhealthy }
                                .thenBy { it.isRunning }
                                .thenBy { it.def.shown },
                        ),
                    )
                is CommsResult.Failed -> FleetLoad.Failed(result.reason)
                CommsResult.Expired -> {
                    state.notice("Signed out — the session expired.")
                    state.signOut(account)
                    FleetLoad.Failed("signed out")
                }
            }
    }

    LaunchedEffect(instance.id) { load() }

    // The ledger lives here because this is the "what is my workforce doing"
    // surface, and spend is the same question asked about money. It does NOT
    // get its own tab: it is one line, and it is information rather than
    // action.
    LaunchedEffect(instance.id, "cost") {
        val read = state.ports.cost(instance, credential)
        // An expired session is the roster read's business — it already signs
        // the account out. The ledger just stops asking.
        if (read is CostResult.Expired) return@LaunchedEffect
        cost = read
    }

    fun control(
        agent: FleetAgent,
        action: AgentAction,
    ) {
        acting = agent.def.id
        error = null
        scope.launch {
            when (val result = state.ports.control(instance, credential, agent.def.id, action)) {
                is CommsResult.Ok -> load() // container reality changed; re-read it
                is CommsResult.Failed -> error = result.reason
                CommsResult.Expired -> {
                    state.notice("Signed out — the session expired.")
                    state.signOut(account)
                }
            }
            acting = null
        }
    }

    Column(Modifier.fillMaxWidth().testTag(TAG_FLEET), verticalArrangement = Arrangement.spacedBy(8.dp)) {
        error?.let {
            Text(
                it,
                color = Mercury.Dark.danger,
                style = MaterialTheme.typography.bodySmall,
                modifier = Modifier.testTag(TAG_FLEET_ERROR),
            )
        }

        when (val ledger = cost) {
            is CostResult.Ok -> {
                val today = ledger.overview.totals.today
                Text(
                    "today: ${'$'}${format2(today.cost)} · ${today.tokens} tokens · ${today.generations} generations",
                    style = MaterialTheme.typography.labelMedium,
                    color = Mercury.Dark.readout,
                    modifier = Modifier.testTag(TAG_COST),
                )
            }
            // Not an error. Most members do not hold the Observability view,
            // and the permission model working as designed should not read as
            // something broken.
            CostResult.Forbidden ->
                Text(
                    "Spend is not shown for this account.",
                    style = MaterialTheme.typography.labelSmall,
                    color = Mercury.Dark.inkDim,
                    modifier = Modifier.testTag(TAG_COST_FORBIDDEN),
                )
            is CostResult.Failed, CostResult.Expired, null -> Unit
        }

        when (val current = load) {
            FleetLoad.Busy -> CircularProgressIndicator(modifier = Modifier.height(24.dp))
            is FleetLoad.Failed ->
                Text(
                    current.reason,
                    color = Mercury.Dark.danger,
                    style = MaterialTheme.typography.bodySmall,
                    modifier = Modifier.testTag(TAG_FLEET_ERROR),
                )
            is FleetLoad.Ready ->
                if (current.agents.isEmpty()) {
                    Text(
                        "No agents on this instance.",
                        color = Mercury.Dark.inkDim,
                        style = MaterialTheme.typography.bodySmall,
                        modifier = Modifier.testTag(TAG_FLEET_EMPTY),
                    )
                } else {
                    LazyColumn(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                        items(current.agents, key = { it.def.id }) { agent ->
                            AgentRow(agent, acting == agent.def.id) { action -> control(agent, action) }
                        }
                    }
                }
        }
    }
}

@Composable
private fun AgentRow(
    agent: FleetAgent,
    busy: Boolean,
    control: (AgentAction) -> Unit,
) {
    Card(Modifier.fillMaxWidth().testTag(TAG_FLEET_ROW)) {
        Column(Modifier.padding(12.dp), verticalArrangement = Arrangement.spacedBy(6.dp)) {
            Row(
                Modifier.fillMaxWidth(),
                horizontalArrangement = Arrangement.SpaceBetween,
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Row(horizontalArrangement = Arrangement.spacedBy(8.dp), verticalAlignment = Alignment.CenterVertically) {
                    // Mercury's semantic rule, exactly: healthy green,
                    // degraded gold, failure orange. Never decorative.
                    Surface(
                        color = dotColor(agent),
                        shape = CircleShape,
                        modifier = Modifier.size(8.dp).testTag(TAG_FLEET_DOT),
                        content = {},
                    )
                    Column {
                        Text(
                            agent.def.shown,
                            color = Mercury.Dark.readout,
                            style = MaterialTheme.typography.titleSmall,
                        )
                        Text(
                            agent.container?.status ?: "stopped",
                            color = Mercury.Dark.muted,
                            style = MaterialTheme.typography.labelSmall,
                            fontFamily = FontFamily.Monospace,
                        )
                    }
                }
                if (busy) CircularProgressIndicator(modifier = Modifier.height(16.dp))
            }

            if (agent.isUnhealthy) {
                Text(
                    "running but failing its healthcheck",
                    color = Mercury.Dark.danger,
                    style = MaterialTheme.typography.labelSmall,
                    modifier = Modifier.testTag(TAG_FLEET_UNHEALTHY),
                )
            }

            if (!agent.def.managed) {
                // Nothing to start or stop. Offering buttons that cannot work
                // is worse than offering none.
                Text(
                    "not a managed service — no container to control",
                    color = Mercury.Dark.inkDim,
                    style = MaterialTheme.typography.labelSmall,
                    modifier = Modifier.testTag(TAG_FLEET_UNMANAGED),
                )
                return@Column
            }

            Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                if (agent.isRunning) {
                    // Stopping an agent is destructive in the sense that
                    // matters — work in flight ends — so it is an orange
                    // outline, never an orange fill.
                    OutlinedButton(
                        onClick = { control(AgentAction.Stop) },
                        enabled = !busy,
                        border = androidx.compose.foundation.BorderStroke(1.dp, SolidColor(Mercury.Dark.danger)),
                        modifier = Modifier.testTag(TAG_FLEET_STOP),
                    ) { Text("Stop", color = Mercury.Dark.danger) }
                    TextButton(
                        onClick = { control(AgentAction.Restart) },
                        enabled = !busy,
                        modifier = Modifier.testTag(TAG_FLEET_RESTART),
                    ) { Text("Restart", color = Mercury.Dark.muted) }
                } else {
                    TextButton(
                        onClick = { control(AgentAction.Up) },
                        enabled = !busy,
                        modifier = Modifier.testTag(TAG_FLEET_START),
                    ) { Text("Start", color = Mercury.accent) }
                }
            }
        }
    }
}

/** Two decimal places without a platform formatter — `kotlin.text` has no
 *  `%.2f` in common code, and money that renders as 4.199999 is money nobody
 *  trusts. */
private fun format2(value: Double): String {
    val cents = kotlin.math.round(value * 100).toLong()
    return "${cents / 100}.${(cents % 100).toString().padStart(2, '0')}"
}

private fun dotColor(agent: FleetAgent): Color =
    when {
        agent.isUnhealthy -> Mercury.Dark.danger
        agent.isStarting -> Mercury.Dark.warning
        agent.isRunning -> Mercury.Dark.success
        else -> Mercury.Dark.inkDim
    }
