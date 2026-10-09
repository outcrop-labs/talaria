package app.talaria.mobile.screens

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material3.Card
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Scaffold
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
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.unit.dp
import app.talaria.mobile.AppState
import app.talaria.mobile.Route
import app.talaria.mobile.SignedTab
import app.talaria.mobile.Tags
import app.talaria.mobile.account.Account
import app.talaria.mobile.mercury.Mercury
import app.talaria.mobile.net.HomeResult
import app.talaria.mobile.net.HomeSummary
import app.talaria.mobile.net.QueueBucket
import app.talaria.mobile.net.WorkItem

/**
 * The landing surface: what is waiting on this account.
 *
 * The three queues ARE the decision queue — review gate, blocked, triage —
 * read from `GET /api/home`. They lead, and unread trails, because a message is
 * not a decision and a badge that conflates the two stops meaning anything.
 */
private sealed interface Load {
    data object Busy : Load

    data class Ready(val summary: HomeSummary) : Load

    data class Failed(val reason: String) : Load

    data object SignedOut : Load
}

@Composable
fun SignedScreen(
    state: AppState,
    account: Account,
) {
    val instance = state.instanceOf(account)
    var load by remember { mutableStateOf<Load>(Load.Busy) }

    LaunchedEffect(account.key) {
        val credential = state.credential(account)
        if (instance == null || credential == null) {
            load = Load.SignedOut
            return@LaunchedEffect
        }
        load =
            when (val home = state.ports.home(instance, credential)) {
                is HomeResult.Ok -> Load.Ready(home.summary)
                is HomeResult.Failed -> Load.Failed(home.reason)
                // An expired session is not an error to apologise for. It is a
                // 7-day absolute TTL doing exactly what it says (gap 1), so the
                // account is dropped and the person is told to sign in again.
                HomeResult.Expired -> {
                    // signOut navigates to the launcher, so a message rendered
                    // here would never be seen. The notice travels instead.
                    state.notice("${account.shown} was signed out — the session expired.")
                    state.signOut(account)
                    Load.SignedOut
                }
            }
    }

    Scaffold(containerColor = Mercury.Dark.ground) { padding ->
        Column(
            modifier = Modifier.fillMaxSize().padding(padding).padding(16.dp).testTag(Tags.HOME),
            verticalArrangement = Arrangement.spacedBy(12.dp),
        ) {
            Row(
                modifier = Modifier.fillMaxWidth(),
                horizontalArrangement = Arrangement.SpaceBetween,
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Column {
                    Text(
                        account.shown,
                        style = MaterialTheme.typography.titleMedium,
                        color = Mercury.Dark.readout,
                        modifier = Modifier.testTag(Tags.ACCOUNT_NAME),
                    )
                    Text(
                        instance?.label ?: "unknown instance",
                        style = MaterialTheme.typography.bodySmall,
                        color = Mercury.Dark.muted,
                    )
                }
                Row {
                    TextButton(
                        onClick = { state.go(Route.Launcher) },
                        modifier = Modifier.testTag(Tags.SWITCH_ACCOUNT),
                    ) { Text("Switch", color = Mercury.Dark.muted) }
                    TextButton(
                        onClick = { state.signOut(account) },
                        modifier = Modifier.testTag(Tags.SIGN_OUT),
                    ) { Text("Sign out", color = Mercury.Dark.danger) }
                }
            }

            // The tab strip. Two surfaces for now — decide, and talk — which
            // are the two things docs/MOBILE.md says a controller is for.
            if (instance != null && state.credential(account) != null) {
                Row(horizontalArrangement = Arrangement.spacedBy(4.dp)) {
                    SignedTab.entries.forEach { tab ->
                        TextButton(
                            onClick = { state.show(tab) },
                            modifier = Modifier.testTag(tabTag(tab)),
                        ) {
                            Text(
                                tab.name,
                                color = if (state.tab == tab) Mercury.accent else Mercury.Dark.muted,
                                style = MaterialTheme.typography.labelLarge,
                            )
                        }
                    }
                }
            }

            if (state.tab == SignedTab.Comms && instance != null) {
                val credential = state.credential(account)
                if (credential != null) {
                    CommsScreen(state, account, instance, credential)
                    return@Column
                }
            }

            when (val current = load) {
                Load.Busy -> CircularProgressIndicator(modifier = Modifier.height(24.dp))
                Load.SignedOut ->
                    Text(
                        "This account is signed out. Sign in again from Instances.",
                        color = Mercury.Dark.muted,
                        style = MaterialTheme.typography.bodyMedium,
                        modifier = Modifier.testTag(Tags.SIGN_IN_ERROR),
                    )
                is Load.Failed ->
                    Text(
                        current.reason,
                        color = Mercury.Dark.danger,
                        style = MaterialTheme.typography.bodySmall,
                        modifier = Modifier.testTag(Tags.ADD_ERROR),
                    )
                is Load.Ready -> HomeBody(current.summary)
            }
        }
    }
}

@Composable
private fun HomeBody(summary: HomeSummary) {
    Text(
        if (summary.needsYou == 0) {
            "Nothing is waiting on you."
        } else {
            "${summary.needsYou} waiting on you"
        },
        style = MaterialTheme.typography.headlineSmall,
        color = if (summary.needsYou == 0) Mercury.Dark.muted else Mercury.Dark.readout,
        modifier = Modifier.testTag(TAG_NEEDS_YOU),
    )

    LazyColumn(verticalArrangement = Arrangement.spacedBy(10.dp)) {
        // Review first, deliberately: clearing the gate is the five-second
        // decision the whole app is for. Blocked is next because an agent is
        // stopped until a person moves. Triage last — nothing is waiting on it.
        item { Queue("Review", summary.queues.review, Mercury.Dark.warning, TAG_REVIEW) }
        item { Queue("Blocked", summary.queues.blocked, Mercury.Dark.danger, TAG_BLOCKED) }
        item { Queue("Triage", summary.queues.triage, Mercury.Dark.muted, TAG_TRIAGE) }
        item {
            Text(
                "${summary.unread} unread · ${summary.boards} boards",
                style = MaterialTheme.typography.bodySmall,
                color = Mercury.Dark.inkDim,
                modifier = Modifier.testTag(TAG_TRAILER),
            )
        }
    }
}

@Composable
private fun Queue(
    name: String,
    bucket: QueueBucket,
    accent: Color,
    tag: String,
) {
    if (bucket.count == 0) return
    Column(verticalArrangement = Arrangement.spacedBy(4.dp), modifier = Modifier.testTag(tag)) {
        Text(
            "$name · ${bucket.count}",
            style = MaterialTheme.typography.labelLarge,
            color = accent,
        )
        bucket.items.forEach { item -> ItemRow(item) }
        // The count is the queue; the items are a window onto it. Saying so
        // beats a list that silently stops short.
        if (bucket.count > bucket.items.size) {
            Text(
                "+${bucket.count - bucket.items.size} more",
                style = MaterialTheme.typography.labelSmall,
                color = Mercury.Dark.inkDim,
            )
        }
    }
}

@Composable
private fun ItemRow(item: WorkItem) {
    Card(modifier = Modifier.fillMaxWidth().testTag(TAG_WORK_ITEM)) {
        Column(Modifier.padding(10.dp), verticalArrangement = Arrangement.spacedBy(2.dp)) {
            Text(item.label, color = Mercury.Dark.readout, style = MaterialTheme.typography.bodyMedium)
            Text(
                "${item.board} · ${item.status}",
                color = Mercury.Dark.muted,
                style = MaterialTheme.typography.labelSmall,
                fontFamily = FontFamily.Monospace,
            )
        }
    }
}

/** One tag per tab, so a test names the surface it is switching to rather than
 *  its position in a row. */
fun tabTag(tab: SignedTab) = "tab-${tab.name.lowercase()}"

const val TAG_NEEDS_YOU = "needs-you"
const val TAG_REVIEW = "queue-review"
const val TAG_BLOCKED = "queue-blocked"
const val TAG_TRIAGE = "queue-triage"
const val TAG_WORK_ITEM = "work-item"
const val TAG_TRAILER = "home-trailer"
