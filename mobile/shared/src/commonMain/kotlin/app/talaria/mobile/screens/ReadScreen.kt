package app.talaria.mobile.screens

import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material3.Card
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
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
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.unit.dp
import app.talaria.mobile.AppState
import app.talaria.mobile.account.Account
import app.talaria.mobile.account.Credential
import app.talaria.mobile.account.Instance
import app.talaria.mobile.mercury.Mercury
import app.talaria.mobile.net.CommsResult
import app.talaria.mobile.net.KbSearchHit
import app.talaria.mobile.net.PlanSummary
import app.talaria.mobile.text.BlockKind
import app.talaria.mobile.text.MarkdownBlock
import app.talaria.mobile.text.parseMarkdown
import app.talaria.mobile.text.snippetToAnnotated
import kotlinx.coroutines.launch

/**
 * Reading: search the knowledgebase, or open a plan's living document.
 *
 * The surface for the moment a decision needs context the queue did not carry.
 * Reading only — editing a versioned markdown drive from a phone means a diff,
 * a merge story and an undo story, which is a desk.
 */
const val TAG_READ = "read"
const val TAG_READ_QUERY = "read-query"
const val TAG_READ_HIT = "read-hit"
const val TAG_READ_NO_HITS = "read-no-hits"
const val TAG_READ_BODY = "read-body"
const val TAG_READ_TITLE = "read-title"
const val TAG_READ_BACK = "read-back"
const val TAG_READ_ERROR = "read-error"
const val TAG_PLAN_ROW = "plan-row"
const val TAG_PLANS_EMPTY = "plans-empty"
const val TAG_PLAN_WORKING = "plan-working"
const val TAG_PLAN_FAILED = "plan-failed"

private sealed interface Open {
    data object Nothing : Open

    data class Doc(
        val title: String,
        val body: String,
    ) : Open
}

@Composable
fun ReadScreen(
    state: AppState,
    account: Account,
    instance: Instance,
    credential: Credential,
) {
    var query by remember { mutableStateOf("") }
    var hits by remember { mutableStateOf<List<KbSearchHit>?>(null) }
    var plans by remember { mutableStateOf<List<PlanSummary>>(emptyList()) }
    var open by remember { mutableStateOf<Open>(Open.Nothing) }
    var busy by remember { mutableStateOf(false) }
    var error by remember { mutableStateOf<String?>(null) }
    val scope = rememberCoroutineScope()

    fun <T> handle(
        result: CommsResult<T>,
        onOk: (T) -> Unit,
    ) {
        when (result) {
            is CommsResult.Ok -> onOk(result.value)
            is CommsResult.Failed -> error = result.reason
            CommsResult.Expired -> {
                state.notice("Signed out — the session expired.")
                state.signOut(account)
            }
        }
    }

    // Plans are listed on arrival; search waits to be asked. A search with no
    // query is not a search, and firing one on every keystroke would spend
    // somebody's battery full-text-searching "t", "th", "the".
    LaunchedEffect(instance.id) {
        handle(state.ports.plans(instance, credential)) { plans = it }
    }

    fun search() {
        val q = query.trim()
        if (q.isEmpty()) return
        busy = true
        error = null
        scope.launch {
            handle(state.ports.searchKnowledge(instance, credential, q)) { hits = it }
            busy = false
        }
    }

    fun openDoc(hit: KbSearchHit) {
        busy = true
        error = null
        scope.launch {
            handle(state.ports.kbDoc(instance, credential, hit.id)) {
                open = Open.Doc(it.title, it.body)
            }
            busy = false
        }
    }

    fun openPlan(plan: PlanSummary) {
        busy = true
        error = null
        scope.launch {
            handle(state.ports.planDoc(instance, credential, plan.id)) {
                open = Open.Doc(it.title.ifBlank { plan.shown }, it.body)
            }
            busy = false
        }
    }

    Column(Modifier.fillMaxWidth().testTag(TAG_READ), verticalArrangement = Arrangement.spacedBy(8.dp)) {
        error?.let {
            Text(
                it,
                color = Mercury.Dark.danger,
                style = MaterialTheme.typography.bodySmall,
                modifier = Modifier.testTag(TAG_READ_ERROR),
            )
        }

        when (val current = open) {
            is Open.Doc -> {
                Row(verticalAlignment = Alignment.CenterVertically) {
                    TextButton(
                        onClick = { open = Open.Nothing },
                        modifier = Modifier.testTag(TAG_READ_BACK),
                    ) { Text("← Read", color = Mercury.Dark.muted) }
                    if (busy) CircularProgressIndicator(modifier = Modifier.height(16.dp))
                }
                Text(
                    current.title,
                    style = MaterialTheme.typography.titleMedium,
                    color = Mercury.Dark.readout,
                    modifier = Modifier.testTag(TAG_READ_TITLE),
                )
                Document(current.body)
            }

            Open.Nothing -> {
                Row(
                    Modifier.fillMaxWidth(),
                    horizontalArrangement = Arrangement.spacedBy(8.dp),
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    OutlinedTextField(
                        value = query,
                        onValueChange = { query = it },
                        placeholder = { Text("Search knowledge") },
                        singleLine = true,
                        modifier = Modifier.fillMaxWidth(0.72f).testTag(TAG_READ_QUERY),
                    )
                    TextButton(onClick = ::search, enabled = query.isNotBlank() && !busy) {
                        Text("Search", color = Mercury.accent)
                    }
                }

                hits?.let { found ->
                    if (found.isEmpty()) {
                        Text(
                            "Nothing matched.",
                            color = Mercury.Dark.inkDim,
                            style = MaterialTheme.typography.bodySmall,
                            modifier = Modifier.testTag(TAG_READ_NO_HITS),
                        )
                    }
                }

                LazyColumn(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                    items(hits.orEmpty(), key = { it.id }) { hit ->
                        HitRow(hit) { openDoc(hit) }
                    }

                    if (hits == null && plans.isNotEmpty()) {
                        item {
                            Text(
                                "Plans",
                                style = MaterialTheme.typography.labelLarge,
                                color = Mercury.Dark.muted,
                            )
                        }
                        items(plans, key = { it.id }) { plan ->
                            PlanRow(plan) { openPlan(plan) }
                        }
                    }

                    if (hits == null && plans.isEmpty()) {
                        item {
                            Text(
                                "No plans yet. Search knowledge above.",
                                color = Mercury.Dark.inkDim,
                                style = MaterialTheme.typography.bodySmall,
                                modifier = Modifier.testTag(TAG_PLANS_EMPTY),
                            )
                        }
                    }
                }
            }
        }
    }
}

@Composable
private fun HitRow(
    hit: KbSearchHit,
    onOpen: () -> Unit,
) {
    Card(Modifier.fillMaxWidth().testTag(TAG_READ_HIT).clickable(onClick = onOpen)) {
        Column(Modifier.padding(10.dp), verticalArrangement = Arrangement.spacedBy(2.dp)) {
            Text(
                listOfNotNull(hit.icon, hit.title).joinToString(" "),
                color = Mercury.Dark.readout,
                style = MaterialTheme.typography.titleSmall,
            )
            Text(hit.spaceName, color = Mercury.Dark.inkDim, style = MaterialTheme.typography.labelSmall)
            // The snippet is the api's pre-escaped HTML with <b> highlights;
            // drawing it raw would show a reader literal tags.
            Text(
                snippetToAnnotated(hit.snippet),
                color = Mercury.Dark.muted,
                style = MaterialTheme.typography.bodySmall,
            )
        }
    }
}

@Composable
private fun PlanRow(
    plan: PlanSummary,
    onOpen: () -> Unit,
) {
    Card(Modifier.fillMaxWidth().testTag(TAG_PLAN_ROW).clickable(onClick = onOpen)) {
        Column(Modifier.padding(10.dp), verticalArrangement = Arrangement.spacedBy(2.dp)) {
            Text(plan.shown, color = Mercury.Dark.readout, style = MaterialTheme.typography.titleSmall)
            Row(horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                Text(plan.agentModel, color = Mercury.Dark.inkDim, style = MaterialTheme.typography.labelSmall)
                plan.ownerLabel?.let {
                    Text(it, color = Mercury.Dark.inkDim, style = MaterialTheme.typography.labelSmall)
                }
                // Gold for working, orange for a turn that errored — the
                // spec's semantic rule, not decoration.
                if (plan.working) {
                    Text(
                        "working",
                        color = Mercury.Dark.warning,
                        style = MaterialTheme.typography.labelSmall,
                        modifier = Modifier.testTag(TAG_PLAN_WORKING),
                    )
                }
                if (plan.failed) {
                    Text(
                        "needs a re-run",
                        color = Mercury.Dark.danger,
                        style = MaterialTheme.typography.labelSmall,
                        modifier = Modifier.testTag(TAG_PLAN_FAILED),
                    )
                }
            }
        }
    }
}

/** The document, through the markdown subset. Anything the subset does not
 *  know degrades to its own source line rather than disappearing. */
@Composable
private fun Document(body: String) {
    val blocks = remember(body) { parseMarkdown(body) }
    LazyColumn(
        verticalArrangement = Arrangement.spacedBy(4.dp),
        modifier = Modifier.fillMaxWidth().height(420.dp).testTag(TAG_READ_BODY),
    ) {
        items(blocks.size) { index -> Block(blocks[index]) }
    }
}

@Composable
private fun Block(block: MarkdownBlock) {
    val indent = (block.indent * 12).dp
    when (block.kind) {
        BlockKind.Heading1 ->
            Text(
                block.text,
                style = MaterialTheme.typography.titleLarge,
                color = Mercury.Dark.readout,
                modifier = Modifier.padding(top = 8.dp),
            )
        BlockKind.Heading2 ->
            Text(
                block.text,
                style = MaterialTheme.typography.titleMedium,
                color = Mercury.Dark.readout,
                modifier = Modifier.padding(top = 6.dp),
            )
        BlockKind.Heading3 ->
            Text(block.text, style = MaterialTheme.typography.titleSmall, color = Mercury.Dark.readout)
        BlockKind.Bullet ->
            Row(Modifier.padding(start = indent), horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                Text("·", color = Mercury.accent, style = MaterialTheme.typography.bodyMedium)
                Text(block.text, color = Mercury.Dark.muted, style = MaterialTheme.typography.bodyMedium)
            }
        BlockKind.Numbered ->
            Row(Modifier.padding(start = indent), horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                Text("—", color = Mercury.accent, style = MaterialTheme.typography.bodyMedium)
                Text(block.text, color = Mercury.Dark.muted, style = MaterialTheme.typography.bodyMedium)
            }
        BlockKind.Quote ->
            Text(
                block.text,
                color = Mercury.Dark.inkDim,
                style = MaterialTheme.typography.bodyMedium,
                modifier = Modifier.padding(start = indent + 8.dp),
            )
        BlockKind.Code ->
            Text(
                block.text,
                color = Mercury.Dark.muted,
                style = MaterialTheme.typography.bodySmall,
                fontFamily = FontFamily.Monospace,
            )
        BlockKind.Paragraph ->
            Text(block.text, color = Mercury.Dark.muted, style = MaterialTheme.typography.bodyMedium)
        BlockKind.Blank -> Text("", style = MaterialTheme.typography.labelSmall)
    }
}
