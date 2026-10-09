package app.talaria.mobile.net

import app.talaria.mobile.account.Credential
import app.talaria.mobile.account.Instance
import io.ktor.client.HttpClient
import io.ktor.client.call.body
import io.ktor.client.request.get
import io.ktor.client.request.post
import io.ktor.client.request.setBody
import io.ktor.client.statement.HttpResponse
import io.ktor.http.ContentType
import io.ktor.http.HttpStatusCode
import io.ktor.http.contentType
import kotlinx.serialization.SerialName
import kotlinx.serialization.Serializable

/**
 * Comms: channels, DMs, and the messages in them.
 *
 * Shapes transcribed from `api/crates/talaria-channels/src/lib.rs` —
 * `MemberChannel` for the list and `ChannelMessageWire` for a page, both
 * `rename_all = "camelCase"`. The envelope is the house one: a read returns a
 * named wrapper (`{channels}`, `{messages}`, `{message}`), never a bare array
 * ([`docs/API-CONVENTIONS.md`](../../../../../../../docs/API-CONVENTIONS.md)).
 *
 * MODELLED: what a phone renders. `guard` is on the wire for a human reader and
 * is deliberately absent here — it carries a guardrail's verdict plus a
 * verbatim excerpt of flagged content, which is a reviewing surface rather than
 * a reading one, and the app's scope line says a surface needing a diff is not
 * a phone surface. `attachments` and `chips` are likewise unmodelled until the
 * screens that render them exist.
 */
@Serializable
data class ChannelPeer(
    @SerialName("userId") val userId: String = "",
    val name: String? = null,
    val email: String? = null,
) {
    val shown: String get() = name?.takeIf { it.isNotBlank() } ?: email ?: userId
}

@Serializable
data class Channel(
    val id: String,
    val name: String = "",
    val topic: String? = null,
    /** `channel`, `dm`, `relay` — the api's own vocabulary. */
    val kind: String = "channel",
    val role: String = "",
    @SerialName("unreadCount") val unreadCount: Int = 0,
    /** DMs: the other person. */
    val peer: ChannelPeer? = null,
    /** DMs only, and absent rather than null for a channel. */
    val members: List<ChannelPeer>? = null,
    /** DMs only: the agents seated in it. */
    val agents: List<String>? = null,
    @SerialName("updatedAt") val updatedAt: String? = null,
) {
    val isDm: Boolean get() = kind == "dm"

    /**
     * What to call this row. A channel has a name; a DM is named by whoever is
     * in it, because "dm-7f3a" is not a thing anyone recognises.
     */
    val label: String
        get() =
            when {
                !isDm -> if (name.isNotBlank()) "#$name" else "#channel"
                peer != null -> peer.shown
                !members.isNullOrEmpty() -> members.joinToString(", ") { it.shown }
                name.isNotBlank() -> name
                else -> "Direct message"
            }
}

@Serializable
data class Message(
    val id: String,
    val seq: Int = 0,
    /** `user` or `agent` — who is talking, and the reason an agent's line can
     *  be shown as an agent's line rather than as a person's. */
    @SerialName("authorType") val authorType: String = "user",
    val author: String = "",
    val content: String = "",
    val status: String = "",
    @SerialName("createdAt") val createdAt: String? = null,
    @SerialName("threadRootId") val threadRootId: String? = null,
    @SerialName("editedAt") val editedAt: String? = null,
) {
    val fromAgent: Boolean get() = authorType == "agent"

    /** A thread reply belongs under its root, not in the main flow. */
    val isThreadReply: Boolean get() = threadRootId != null
}

@Serializable
private data class ChannelsEnvelope(val channels: List<Channel> = emptyList())

@Serializable
private data class MessagesEnvelope(val messages: List<Message> = emptyList())

@Serializable
private data class MessageEnvelope(val message: Message)

@Serializable
private data class PostBody(val content: String)

const val CHANNELS_PATH = "/api/channels"

fun channelMessagesPath(channelId: String) = "/api/channels/$channelId/messages"

fun channelReadPath(channelId: String) = "/api/channels/$channelId/read"

sealed interface CommsResult<out T> {
    data class Ok<T>(val value: T) : CommsResult<T>

    data object Expired : CommsResult<Nothing>

    data class Failed(val reason: String) : CommsResult<Nothing>
}

/** One place that turns a response into a result, so every comms call reports
 *  an expired session the same way and none of them swallow it. */
private suspend fun <T> guarded(
    instance: Instance,
    call: suspend () -> HttpResponse,
    decode: suspend (HttpResponse) -> T,
): CommsResult<T> {
    val response =
        try {
            call()
        } catch (e: Throwable) {
            return CommsResult.Failed("could not reach ${instance.label}: ${e.message ?: "no answer"}")
        }
    if (response.status == HttpStatusCode.Unauthorized || response.status == HttpStatusCode.Forbidden) {
        return CommsResult.Expired
    }
    if (!response.status.isSuccess()) {
        return CommsResult.Failed("${instance.label} answered ${response.status.value}")
    }
    return try {
        CommsResult.Ok(decode(response))
    } catch (_: Throwable) {
        CommsResult.Failed("${instance.label} answered something unreadable")
    }
}

private fun HttpStatusCode.isSuccess() = value in 200..299

suspend fun readChannels(
    client: HttpClient,
    instance: Instance,
    credential: Credential,
): CommsResult<List<Channel>> =
    guarded(
        instance,
        call = { client.get("${instance.origin}$CHANNELS_PATH") { credential.applyTo(this) } },
        decode = { it.body<ChannelsEnvelope>().channels },
    )

suspend fun readMessages(
    client: HttpClient,
    instance: Instance,
    credential: Credential,
    channelId: String,
): CommsResult<List<Message>> =
    guarded(
        instance,
        call = {
            client.get("${instance.origin}${channelMessagesPath(channelId)}") { credential.applyTo(this) }
        },
        decode = { it.body<MessagesEnvelope>().messages },
    )

suspend fun postMessage(
    client: HttpClient,
    instance: Instance,
    credential: Credential,
    channelId: String,
    content: String,
): CommsResult<Message> =
    guarded(
        instance,
        call = {
            client.post("${instance.origin}${channelMessagesPath(channelId)}") {
                credential.applyTo(this)
                contentType(ContentType.Application.Json)
                setBody(PostBody(content))
            }
        },
        decode = { it.body<MessageEnvelope>().message },
    )

/** Mark a channel read. Fire-and-forget by nature: failing to clear a badge is
 *  not worth interrupting someone who is already reading the messages. */
suspend fun markRead(
    client: HttpClient,
    instance: Instance,
    credential: Credential,
    channelId: String,
): CommsResult<Unit> =
    guarded(
        instance,
        call = { client.post("${instance.origin}${channelReadPath(channelId)}") { credential.applyTo(this) } },
        decode = { },
    )
