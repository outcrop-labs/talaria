package app.talaria.mobile.screens

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.material3.Button
import androidx.compose.material3.Card
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
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
import app.talaria.mobile.AppState
import app.talaria.mobile.account.Account
import app.talaria.mobile.account.Credential
import app.talaria.mobile.account.Instance
import app.talaria.mobile.mercury.Mercury
import app.talaria.mobile.net.ChatEvent
import app.talaria.mobile.net.ToolCall
import app.talaria.mobile.net.ToolStatus
import app.talaria.mobile.net.mergeTool
import kotlinx.coroutines.launch

/**
 * The assistant: say what you want, watch it happen.
 *
 * This is the surface a phone does BETTER than a desk, which is why it earns
 * room in a controller app at all — dictating "ask engineering why the deploy
 * stalled and start a draft" while walking is not something a laptop improves.
 *
 * A turn shows three different kinds of thing and keeps them apart, because
 * conflating them is how a transcript becomes unreadable: the ANSWER, the
 * model's own REASONING, and the TOOLS an agent ran. An agent narrating its
 * tool use is the main reason a reply takes thirty seconds, so hiding it would
 * make a working assistant look frozen.
 */
const val TAG_ASSISTANT = "assistant"
const val TAG_ASSISTANT_COMPOSER = "assistant-composer"
const val TAG_ASSISTANT_SEND = "assistant-send"
const val TAG_ASSISTANT_TURN = "assistant-turn"
const val TAG_ASSISTANT_REASONING = "assistant-reasoning"
const val TAG_ASSISTANT_TOOL = "assistant-tool"
const val TAG_ASSISTANT_USAGE = "assistant-usage"
const val TAG_ASSISTANT_ERROR = "assistant-error"
const val TAG_ASSISTANT_NO_MODEL = "assistant-no-model"
const val TAG_ASSISTANT_QUEUED = "assistant-queued"
const val TAG_ASSISTANT_NEW = "assistant-new"

/** One turn in the transcript. The assistant's half grows as tokens land. */
private data class Turn(
    val id: Int,
    val fromMe: Boolean,
    val text: String = "",
    val reasoning: String = "",
    val tools: List<ToolCall> = emptyList(),
    val usage: ChatEvent.Usage? = null,
    val queued: Boolean = false,
)

@Composable
fun AssistantScreen(
    state: AppState,
    account: Account,
    instance: Instance,
    credential: Credential,
) {
    val model = account.preferredModel

    if (model.isNullOrBlank()) {
        // Picking a model is configuration, and configuration is a desk
        // surface. Guessing one would quietly spend somebody's money on a tier
        // they did not choose, so the app says what it needs instead.
        //
        // Rendered INSIDE the surface's own container rather than returning
        // early: a tab that renders a bare sentence with no frame around it
        // reads as broken rather than as explaining itself.
        Column(
            Modifier.fillMaxWidth().testTag(TAG_ASSISTANT),
            verticalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            Text(
                "No preferred model set for this account. Choose one on the web app " +
                    "(Settings → model) and it will appear here.",
                style = MaterialTheme.typography.bodyMedium,
                color = Mercury.Dark.muted,
                modifier = Modifier.testTag(TAG_ASSISTANT_NO_MODEL),
            )
        }
        return
    }

    val turns = remember { mutableStateListOf<Turn>() }
    var draft by remember { mutableStateOf("") }
    var streaming by remember { mutableStateOf(false) }
    var error by remember { mutableStateOf<String?>(null) }
    var conversationId by remember { mutableStateOf<String?>(null) }
    var nextId by remember { mutableStateOf(0) }
    val scope = rememberCoroutineScope()
    val listState = rememberLazyListState()

    // Tokens land at the bottom, so that is where the eye should be.
    LaunchedEffect(turns.size, turns.lastOrNull()?.text?.length) {
        if (turns.isNotEmpty()) listState.animateScrollToItem(turns.size - 1)
    }

    fun send() {
        val content = draft.trim()
        if (content.isEmpty()) return
        draft = ""
        error = null
        streaming = true

        turns += Turn(id = nextId++, fromMe = true, text = content)
        val reply = Turn(id = nextId++, fromMe = false)
        turns += reply
        val replyIndex = turns.lastIndex

        scope.launch {
            // Each event rewrites the reply in place. A data class copy into the
            // snapshot list is what makes Compose see the change — mutating the
            // instance alone would not recompose.
            fun update(block: (Turn) -> Turn) {
                turns[replyIndex] = block(turns[replyIndex])
            }

            state.ports
                .chat(instance, credential, model, content, conversationId)
                .collect { event ->
                    when (event) {
                        // The id arrives before any token, and keeping it is
                        // what makes the NEXT turn continue this thread rather
                        // than starting a new one.
                        is ChatEvent.Started -> event.conversationId?.let { conversationId = it }
                        is ChatEvent.Content -> update { it.copy(text = it.text + event.text) }
                        is ChatEvent.Reasoning -> update { it.copy(reasoning = it.reasoning + event.text) }
                        is ChatEvent.Tool -> update { it.copy(tools = mergeTool(it.tools, event)) }
                        is ChatEvent.Usage -> update { it.copy(usage = event) }
                        ChatEvent.Queued -> update { it.copy(queued = true) }
                        is ChatEvent.Failed -> error = event.reason
                        ChatEvent.Expired -> {
                            state.notice("Signed out — the session expired.")
                            state.signOut(account)
                        }
                    }
                }
            streaming = false
        }
    }

    Column(Modifier.fillMaxWidth().testTag(TAG_ASSISTANT), verticalArrangement = Arrangement.spacedBy(8.dp)) {
        Row(verticalAlignment = Alignment.CenterVertically) {
            Text(
                model,
                style = MaterialTheme.typography.labelSmall,
                color = Mercury.Dark.inkDim,
                fontFamily = FontFamily.Monospace,
            )
            if (turns.isNotEmpty()) {
                TextButton(
                    onClick = {
                        // A fresh thread, deliberately explicit. Agent DMs are
                        // "fresh thread per topic" by design here, and silently
                        // continuing one forever is how context rot sets in.
                        turns.clear()
                        conversationId = null
                        error = null
                    },
                    modifier = Modifier.testTag(TAG_ASSISTANT_NEW),
                ) { Text("New thread", color = Mercury.Dark.muted) }
            }
            if (streaming) CircularProgressIndicator(modifier = Modifier.height(16.dp))
        }

        error?.let {
            Text(
                it,
                color = Mercury.Dark.danger,
                style = MaterialTheme.typography.bodySmall,
                modifier = Modifier.testTag(TAG_ASSISTANT_ERROR),
            )
        }

        LazyColumn(
            state = listState,
            verticalArrangement = Arrangement.spacedBy(8.dp),
            modifier = Modifier.fillMaxWidth().height(340.dp),
        ) {
            items(turns, key = { it.id }) { turn -> TurnRow(turn) }
        }

        Row(
            Modifier.fillMaxWidth(),
            horizontalArrangement = Arrangement.spacedBy(8.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            OutlinedTextField(
                value = draft,
                onValueChange = { draft = it },
                placeholder = { Text("Ask, or tell it to do something") },
                modifier = Modifier.fillMaxWidth(0.72f).testTag(TAG_ASSISTANT_COMPOSER),
            )
            Button(
                onClick = ::send,
                enabled = draft.isNotBlank() && !streaming,
                modifier = Modifier.testTag(TAG_ASSISTANT_SEND),
            ) { Text("Send") }
        }
    }
}

@Composable
private fun TurnRow(turn: Turn) {
    Card(Modifier.fillMaxWidth().testTag(TAG_ASSISTANT_TURN)) {
        Column(Modifier.padding(10.dp), verticalArrangement = Arrangement.spacedBy(4.dp)) {
            Text(
                if (turn.fromMe) "you" else "assistant",
                style = MaterialTheme.typography.labelSmall,
                color = if (turn.fromMe) Mercury.Dark.muted else Mercury.accent,
            )

            // Tools first: they are what is happening NOW, and the answer is
            // what happens after.
            turn.tools.forEach { tool ->
                Row(horizontalArrangement = Arrangement.spacedBy(6.dp), modifier = Modifier.testTag(TAG_ASSISTANT_TOOL)) {
                    Text(
                        tool.label.ifEmpty { tool.name },
                        style = MaterialTheme.typography.labelSmall,
                        color = Mercury.Dark.muted,
                    )
                    Text(
                        if (tool.status == ToolStatus.Completed) "done" else "running",
                        style = MaterialTheme.typography.labelSmall,
                        color =
                            if (tool.status == ToolStatus.Completed) {
                                Mercury.Dark.success
                            } else {
                                Mercury.Dark.warning
                            },
                    )
                }
            }

            if (turn.reasoning.isNotEmpty()) {
                Text(
                    turn.reasoning,
                    style = MaterialTheme.typography.labelSmall,
                    color = Mercury.Dark.inkDim,
                    modifier = Modifier.testTag(TAG_ASSISTANT_REASONING),
                )
            }

            if (turn.text.isNotEmpty()) {
                Text(turn.text, style = MaterialTheme.typography.bodyMedium, color = Mercury.Dark.readout)
            }

            if (turn.queued) {
                Text(
                    "Queued — the reply will join the conversation.",
                    style = MaterialTheme.typography.labelSmall,
                    color = Mercury.Dark.muted,
                    modifier = Modifier.testTag(TAG_ASSISTANT_QUEUED),
                )
            }

            turn.usage?.let {
                Text(
                    "${it.promptTokens} in · ${it.completionTokens} out",
                    style = MaterialTheme.typography.labelSmall,
                    color = Mercury.Dark.inkDim,
                    modifier = Modifier.testTag(TAG_ASSISTANT_USAGE),
                )
            }
        }
    }
}
