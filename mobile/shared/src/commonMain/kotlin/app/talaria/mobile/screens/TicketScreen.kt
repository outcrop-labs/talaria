package app.talaria.mobile.screens

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material3.Button
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.Card
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
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
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.unit.dp
import app.talaria.mobile.AppState
import app.talaria.mobile.account.Account
import app.talaria.mobile.account.Credential
import app.talaria.mobile.account.Instance
import app.talaria.mobile.mercury.Mercury
import app.talaria.mobile.net.CommsResult
import app.talaria.mobile.net.ReviewVerdict
import app.talaria.mobile.net.Task
import app.talaria.mobile.net.TaskComment
import app.talaria.mobile.net.TaskDetail
import kotlinx.coroutines.launch

/**
 * One ticket, and the two buttons the whole app exists for.
 *
 * THE GATE'S SEMANTICS ARE THE API'S, not a convenience. Approving **marks**
 * the ticket signed off and leaves it in its column, because a reviewed ticket
 * usually still has a merge or a release ahead of it; rejecting is what moves
 * it, back to the board's first working column. So the buttons say "Approve"
 * and "Send back" rather than anything implying completion, and nothing here
 * can reach `done` — the repo's rule is that a human signs off and a human
 * moves it, and a phone that quietly completed work would undo the guarantee
 * the gate exists to make.
 */
const val TAG_TICKET = "ticket"
const val TAG_TICKET_TITLE = "ticket-title"
const val TAG_TICKET_STATUS = "ticket-status"
const val TAG_APPROVE = "approve"
const val TAG_REJECT = "reject"
const val TAG_REVIEW_NOTES = "review-notes"
const val TAG_APPROVED_MARK = "approved-mark"
const val TAG_COMMENT_BOX = "comment-box"
const val TAG_COMMENT_SEND = "comment-send"
const val TAG_COMMENT = "ticket-comment"
const val TAG_TICKET_BACK = "ticket-back"
const val TAG_TICKET_ERROR = "ticket-error"
const val TAG_GATE = "review-gate"

private sealed interface TicketLoad {
    data object Busy : TicketLoad

    data class Ready(val detail: TaskDetail) : TicketLoad

    data class Failed(val reason: String) : TicketLoad
}

@Composable
fun TicketScreen(
    state: AppState,
    account: Account,
    instance: Instance,
    credential: Credential,
    taskId: String,
) {
    var load by remember { mutableStateOf<TicketLoad>(TicketLoad.Busy) }
    var notes by remember { mutableStateOf("") }
    var draft by remember { mutableStateOf("") }
    var acting by remember { mutableStateOf(false) }
    var actionError by remember { mutableStateOf<String?>(null) }
    val scope = rememberCoroutineScope()

    /** Every write re-reads through the same path the api does, so the screen
     *  shows what the server now believes rather than what we hoped. */
    suspend fun load() {
        load =
            when (val result = state.ports.task(instance, credential, taskId)) {
                is CommsResult.Ok -> TicketLoad.Ready(result.value)
                is CommsResult.Failed -> TicketLoad.Failed(result.reason)
                CommsResult.Expired -> {
                    state.notice("Signed out — the session expired.")
                    state.signOut(account)
                    TicketLoad.Failed("signed out")
                }
            }
    }

    LaunchedEffect(taskId) { load() }

    fun <T> handle(result: CommsResult<T>, onOk: (T) -> Unit) {
        when (result) {
            is CommsResult.Ok -> onOk(result.value)
            is CommsResult.Failed -> actionError = result.reason
            CommsResult.Expired -> {
                state.notice("Signed out — the session expired.")
                state.signOut(account)
            }
        }
    }

    fun decide(verdict: ReviewVerdict) {
        acting = true
        actionError = null
        scope.launch {
            handle(state.ports.review(instance, credential, taskId, verdict, notes)) {
                notes = ""
            }
            // Re-read either way: a rejection moved the ticket, and an approval
            // set a mark that changes what this screen may offer.
            load()
            acting = false
        }
    }

    fun comment() {
        val content = draft.trim()
        if (content.isEmpty()) return
        acting = true
        actionError = null
        scope.launch {
            handle(state.ports.comment(instance, credential, taskId, content)) { draft = "" }
            load()
            acting = false
        }
    }

    Column(Modifier.fillMaxWidth().testTag(TAG_TICKET), verticalArrangement = Arrangement.spacedBy(10.dp)) {
        Row(verticalAlignment = Alignment.CenterVertically) {
            TextButton(
                onClick = { state.openTicket(null) },
                modifier = Modifier.testTag(TAG_TICKET_BACK),
            ) { Text("← Back", color = Mercury.Dark.muted) }
            if (acting) CircularProgressIndicator(modifier = Modifier.height(18.dp))
        }

        when (val current = load) {
            TicketLoad.Busy -> CircularProgressIndicator(modifier = Modifier.height(24.dp))
            is TicketLoad.Failed ->
                Text(
                    current.reason,
                    color = Mercury.Dark.danger,
                    style = MaterialTheme.typography.bodySmall,
                    modifier = Modifier.testTag(TAG_TICKET_ERROR),
                )
            is TicketLoad.Ready -> {
                val task = current.detail.task
                TicketHeader(task)

                actionError?.let {
                    Text(
                        it,
                        color = Mercury.Dark.danger,
                        style = MaterialTheme.typography.bodySmall,
                        modifier = Modifier.testTag(TAG_TICKET_ERROR),
                    )
                }

                ReviewGate(task, notes, { notes = it }, acting, ::decide)
                Comments(current.detail.comments, draft, { draft = it }, acting, ::comment)
            }
        }
    }
}

@Composable
private fun TicketHeader(task: Task) {
    Text(
        task.label,
        style = MaterialTheme.typography.titleMedium,
        color = Mercury.Dark.readout,
        modifier = Modifier.testTag(TAG_TICKET_TITLE),
    )
    Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
        Text(
            task.status,
            style = MaterialTheme.typography.labelSmall,
            color = Mercury.Dark.muted,
            fontFamily = FontFamily.Monospace,
            modifier = Modifier.testTag(TAG_TICKET_STATUS),
        )
        if (task.priority.isNotBlank()) {
            Text(task.priority, style = MaterialTheme.typography.labelSmall, color = Mercury.Dark.inkDim)
        }
        if (task.assignees.isNotEmpty()) {
            Text(
                task.assignees.joinToString(", "),
                style = MaterialTheme.typography.labelSmall,
                color = Mercury.Dark.inkDim,
            )
        }
    }
    task.description?.takeIf { it.isNotBlank() }?.let {
        Text(it, style = MaterialTheme.typography.bodyMedium, color = Mercury.Dark.muted)
    }
    // An agent's own account of how it went is exactly the context a five-second
    // decision needs, so it leads rather than hiding below the comments.
    task.outcome?.takeIf { it.isNotBlank() }?.let {
        Text("outcome: $it", style = MaterialTheme.typography.bodySmall, color = Mercury.Dark.readout)
    }
    task.errorMessage?.takeIf { it.isNotBlank() }?.let {
        Text(it, style = MaterialTheme.typography.bodySmall, color = Mercury.Dark.danger)
    }
}

@Composable
private fun ReviewGate(
    task: Task,
    notes: String,
    onNotes: (String) -> Unit,
    acting: Boolean,
    decide: (ReviewVerdict) -> Unit,
) {
    if (task.isApproved) {
        // Already signed off. Offering the gate again would invite a second
        // approval of the same work and make the queue lie about what is left.
        Text(
            "Signed off${task.approvedAt?.let { " · $it" } ?: ""}",
            style = MaterialTheme.typography.labelMedium,
            color = Mercury.Dark.success,
            modifier = Modifier.testTag(TAG_APPROVED_MARK),
        )
        return
    }

    Column(verticalArrangement = Arrangement.spacedBy(6.dp), modifier = Modifier.testTag(TAG_GATE)) {
        OutlinedTextField(
            value = notes,
            onValueChange = onNotes,
            placeholder = { Text("notes (optional for approve, worth it for a send-back)") },
            modifier = Modifier.fillMaxWidth().testTag(TAG_REVIEW_NOTES),
        )
        Row(horizontalArrangement = Arrangement.spacedBy(10.dp)) {
            // Gold fill for the primary action, dark glyphs on it — the spec's
            // rule, and the accent is the same warm gold as `warning`.
            Button(
                onClick = { decide(ReviewVerdict.Approve) },
                enabled = !acting,
                colors =
                    ButtonDefaults.buttonColors(
                        containerColor = Mercury.accent,
                        contentColor = Mercury.onAccent,
                    ),
                modifier = Modifier.testTag(TAG_APPROVE),
            ) { Text("Approve") }

            // Destructive is an orange OUTLINE, never an orange fill: orange is
            // never decorative in Mercury.
            OutlinedButton(
                onClick = { decide(ReviewVerdict.Reject) },
                enabled = !acting,
                border = androidx.compose.foundation.BorderStroke(1.dp, SolidColor(Mercury.Dark.danger)),
                modifier = Modifier.testTag(TAG_REJECT),
            ) { Text("Send back", color = Mercury.Dark.danger) }
        }
        Text(
            // Said out loud, because the honest semantics are not the obvious
            // ones and someone tapping Approve deserves to know what it does.
            "Approving marks it reviewed and leaves it where it is. Sending it back returns it to the first working column.",
            style = MaterialTheme.typography.labelSmall,
            color = Mercury.Dark.inkDim,
        )
    }
}

@Composable
private fun Comments(
    comments: List<TaskComment>,
    draft: String,
    onDraft: (String) -> Unit,
    acting: Boolean,
    send: () -> Unit,
) {
    if (comments.isNotEmpty()) {
        LazyColumn(
            verticalArrangement = Arrangement.spacedBy(6.dp),
            modifier = Modifier.fillMaxWidth().height(220.dp),
        ) {
            items(comments, key = { it.id }) { comment ->
                Card(Modifier.fillMaxWidth().testTag(TAG_COMMENT)) {
                    Column(Modifier.padding(10.dp), verticalArrangement = Arrangement.spacedBy(2.dp)) {
                        Text(
                            comment.author,
                            style = MaterialTheme.typography.labelSmall,
                            color = Mercury.Dark.muted,
                        )
                        Text(
                            comment.content,
                            style = MaterialTheme.typography.bodyMedium,
                            color = Mercury.Dark.readout,
                        )
                    }
                }
            }
        }
    }
    Row(
        Modifier.fillMaxWidth(),
        horizontalArrangement = Arrangement.spacedBy(8.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        OutlinedTextField(
            value = draft,
            onValueChange = onDraft,
            placeholder = { Text("Comment") },
            modifier = Modifier.fillMaxWidth(0.7f).testTag(TAG_COMMENT_BOX),
        )
        Button(
            onClick = send,
            enabled = draft.isNotBlank() && !acting,
            modifier = Modifier.testTag(TAG_COMMENT_SEND),
        ) { Text("Post") }
    }
}
