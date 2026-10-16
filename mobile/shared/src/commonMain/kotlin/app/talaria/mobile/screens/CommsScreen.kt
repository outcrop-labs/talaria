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
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.material3.Badge
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
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.unit.dp
import app.talaria.mobile.AppState
import app.talaria.mobile.Tags
import app.talaria.mobile.account.Account
import app.talaria.mobile.account.Credential
import app.talaria.mobile.account.Instance
import app.talaria.mobile.mercury.Mercury
import app.talaria.mobile.net.Channel
import app.talaria.mobile.net.CommsResult
import app.talaria.mobile.net.Message
import app.talaria.mobile.net.UserEvent
import kotlinx.coroutines.launch

/**
 * Comms: the rooms this account is in, and the messages in one of them.
 *
 * Channels and DMs are one list rather than two, because on a phone the
 * question is "where is the conversation I need" and not "what kind of room is
 * it". Unread leads the sort — the only ordering a glance can use.
 */
const val TAG_CHANNEL_LIST = "channel-list"
const val TAG_CHANNEL_ROW = "channel-row"
const val TAG_CHANNEL_UNREAD = "channel-unread"
const val TAG_MESSAGE = "message"
const val TAG_COMPOSER = "composer"
const val TAG_SEND = "send"
const val TAG_CHANNEL_TITLE = "channel-title"
const val TAG_COMMS_EMPTY = "comms-empty"
const val TAG_BACK_TO_LIST = "back-to-list"

@Composable
fun CommsScreen(
    state: AppState,
    account: Account,
    instance: Instance,
    credential: Credential,
) {
    val open = state.openChannelId
    if (open == null) {
        ChannelList(state, instance, credential)
    } else {
        ChannelView(state, account, instance, credential, open)
    }
}

@Composable
private fun ChannelList(
    state: AppState,
    instance: Instance,
    credential: Credential,
) {
    var channels by remember { mutableStateOf<List<Channel>?>(null) }
    var error by remember { mutableStateOf<String?>(null) }

    suspend fun load() {
        when (val result = state.ports.channels(instance, credential)) {
            is CommsResult.Ok ->
                // Unread first, then whatever the api's own order was. A phone
                // list that buries the one room with something new in it is a
                // list nobody can use at a glance.
                channels = result.value.sortedByDescending { it.unreadCount }
            is CommsResult.Failed -> error = result.reason
            CommsResult.Expired -> {
                state.notice("Signed out — the session expired.")
                state.signOut(state.accounts.first { it.instanceId == instance.id })
            }
        }
    }

    LaunchedEffect(instance.id) { load() }

    // The firehose, as the list's refresh trigger. A channel event carries an
    // id and nothing else, so the only correct response is to re-read through
    // the ordinary route — which is exactly what the realtime invariant
    // intends, and why no event needs to carry a message.
    LaunchedEffect(instance.id, "events") {
        state.ports.events(instance, credential).collect { event ->
            if (event is UserEvent.Channel) load()
        }
    }

    when {
        error != null ->
            Text(
                error!!,
                color = Mercury.Dark.danger,
                style = MaterialTheme.typography.bodySmall,
                modifier = Modifier.testTag(Tags.ADD_ERROR),
            )
        channels == null -> CircularProgressIndicator(modifier = Modifier.height(24.dp))
        channels!!.isEmpty() ->
            Text(
                "No conversations yet.",
                color = Mercury.Dark.inkDim,
                style = MaterialTheme.typography.bodySmall,
                modifier = Modifier.testTag(TAG_COMMS_EMPTY),
            )
        else ->
            LazyColumn(
                verticalArrangement = Arrangement.spacedBy(8.dp),
                modifier = Modifier.testTag(TAG_CHANNEL_LIST),
            ) {
                items(channels!!, key = { it.id }) { channel ->
                    ChannelRow(channel) { state.openChannel(channel.id) }
                }
            }
    }
}

@Composable
private fun ChannelRow(
    channel: Channel,
    onOpen: () -> Unit,
) {
    Card(modifier = Modifier.fillMaxWidth().testTag(TAG_CHANNEL_ROW).clickable(onClick = onOpen)) {
        Row(
            Modifier.padding(12.dp).fillMaxWidth(),
            horizontalArrangement = Arrangement.SpaceBetween,
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Column(verticalArrangement = Arrangement.spacedBy(2.dp)) {
                Text(
                    channel.label,
                    color = Mercury.Dark.readout,
                    style = MaterialTheme.typography.titleSmall,
                )
                channel.topic?.takeIf { it.isNotBlank() }?.let {
                    Text(it, color = Mercury.Dark.muted, style = MaterialTheme.typography.labelSmall)
                }
            }
            if (channel.unreadCount > 0) {
                Badge(
                    containerColor = Mercury.accent,
                    contentColor = Mercury.onAccent,
                    modifier = Modifier.testTag(TAG_CHANNEL_UNREAD),
                ) { Text("${channel.unreadCount}") }
            }
        }
    }
}

@Composable
private fun ChannelView(
    state: AppState,
    account: Account,
    instance: Instance,
    credential: Credential,
    channelId: String,
) {
    var messages by remember { mutableStateOf<List<Message>?>(null) }
    var error by remember { mutableStateOf<String?>(null) }
    var draft by remember { mutableStateOf("") }
    var sending by remember { mutableStateOf(false) }
    val scope = rememberCoroutineScope()
    val listState = rememberLazyListState()

    suspend fun load() {
        when (val result = state.ports.messages(instance, credential, channelId)) {
            is CommsResult.Ok ->
                // Thread replies hang off their root and do not belong in the
                // main flow; a phone shows the room, not the tree.
                messages = result.value.filterNot { it.isThreadReply }
            is CommsResult.Failed -> error = result.reason
            CommsResult.Expired -> {
                state.notice("Signed out — the session expired.")
                state.signOut(account)
            }
        }
    }

    LaunchedEffect(channelId) {
        load()
        // Opening a room is reading it. The result is ignored on purpose:
        // failing to clear a badge is not worth interrupting someone who is
        // already looking at the messages.
        state.ports.markRead(instance, credential, channelId)
    }

    // Live: a `channel` event for THIS room re-reads it. Other rooms' events
    // are the list's business, not this screen's.
    LaunchedEffect(channelId, "events") {
        state.ports.events(instance, credential).collect { event ->
            if (event is UserEvent.Channel && event.channelId == channelId) load()
        }
    }

    // New messages arrive at the bottom, so that is where the eye should be.
    LaunchedEffect(messages?.size) {
        val count = messages?.size ?: 0
        if (count > 0) listState.animateScrollToItem(count - 1)
    }

    fun send() {
        val content = draft.trim()
        if (content.isEmpty()) return
        sending = true
        scope.launch {
            when (val result = state.ports.post(instance, credential, channelId, content)) {
                is CommsResult.Ok -> {
                    draft = ""
                    // The posted message is appended from the response rather
                    // than waiting for the stream to echo it: a composer that
                    // clears before the line appears feels broken.
                    messages = (messages ?: emptyList()) + result.value
                }
                is CommsResult.Failed -> error = result.reason
                CommsResult.Expired -> {
                    state.notice("Signed out — the session expired.")
                    state.signOut(account)
                }
            }
            sending = false
        }
    }

    Column(Modifier.fillMaxWidth(), verticalArrangement = Arrangement.spacedBy(8.dp)) {
        Row(verticalAlignment = Alignment.CenterVertically) {
            TextButton(
                onClick = { state.openChannel(null) },
                modifier = Modifier.testTag(TAG_BACK_TO_LIST),
            ) { Text("← Comms", color = Mercury.Dark.muted) }
            Text(
                channelId,
                color = Mercury.Dark.inkDim,
                style = MaterialTheme.typography.labelSmall,
                modifier = Modifier.testTag(TAG_CHANNEL_TITLE),
            )
        }

        error?.let {
            Text(
                it,
                color = Mercury.Dark.danger,
                style = MaterialTheme.typography.bodySmall,
                modifier = Modifier.testTag(Tags.ADD_ERROR),
            )
        }

        if (messages == null) {
            CircularProgressIndicator(modifier = Modifier.height(24.dp))
        } else {
            LazyColumn(
                state = listState,
                verticalArrangement = Arrangement.spacedBy(6.dp),
                modifier = Modifier.fillMaxWidth().height(360.dp),
            ) {
                items(messages!!, key = { it.id }) { message -> MessageRow(message) }
            }
        }

        Row(
            Modifier.fillMaxWidth(),
            horizontalArrangement = Arrangement.spacedBy(8.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            OutlinedTextField(
                value = draft,
                onValueChange = { draft = it },
                placeholder = { Text("Message") },
                modifier = Modifier.fillMaxWidth(0.72f).testTag(TAG_COMPOSER),
            )
            Button(
                onClick = ::send,
                enabled = draft.isNotBlank() && !sending,
                modifier = Modifier.testTag(TAG_SEND),
            ) { Text("Send") }
        }
    }
}

@Composable
private fun MessageRow(message: Message) {
    Column(
        Modifier.fillMaxWidth().testTag(TAG_MESSAGE),
        verticalArrangement = Arrangement.spacedBy(1.dp),
    ) {
        Row(horizontalArrangement = Arrangement.spacedBy(6.dp)) {
            Text(
                message.author,
                // An agent is labelled as an agent. Talaria's whole premise is
                // that agents are staff, which only works if you can always
                // tell which of your colleagues is one.
                color = if (message.fromAgent) Mercury.accent else Mercury.Dark.muted,
                style = MaterialTheme.typography.labelSmall,
            )
            if (message.fromAgent) {
                Text("agent", color = Mercury.Dark.inkDim, style = MaterialTheme.typography.labelSmall)
            }
            if (message.editedAt != null) {
                Text("edited", color = Mercury.Dark.inkDim, style = MaterialTheme.typography.labelSmall)
            }
        }
        Text(message.content, color = Mercury.Dark.readout, style = MaterialTheme.typography.bodyMedium)
    }
}
