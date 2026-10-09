package app.talaria.mobile.net

import app.talaria.mobile.account.Credential
import app.talaria.mobile.account.Instance
import io.ktor.client.HttpClient
import io.ktor.client.plugins.HttpTimeoutConfig
import io.ktor.client.plugins.timeout
import io.ktor.client.request.header
import io.ktor.client.request.prepareGet
import io.ktor.client.statement.bodyAsChannel
import io.ktor.utils.io.readUTF8Line
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.flow
import kotlinx.serialization.SerialName
import kotlinx.serialization.Serializable
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonClassDiscriminator

/**
 * `GET /api/me/events` — this person's own firehose, and the reason a run
 * parked on a question at 11pm can reach a phone.
 *
 * THE LOAD-BEARING RULE, and it shapes this whole file: **an event says what
 * changed, never what it says.** Every variant below carries ids and nothing
 * else, and the client re-fetches through the ordinary ACL'd route. So realtime
 * can never widen a read, and a payload cannot leak by growing a field. The api
 * states it in as many words (`docs/ARCHITECTURE.md`, Realtime), and
 * `UserEvent` in `api/crates/talaria-realtime/src/lib.rs` is serialized
 * directly with `tag = "type"`, `rename_all = "lowercase"` and
 * `rename_all_fields = "camelCase"` — which is exactly the discriminator below.
 *
 * AUTHORIZATION IS THE SESSION. There is no `?userId=`; the topic is keyed from
 * the resolved session, so the only firehose this route can construct is the
 * caller's own.
 */
@Serializable
@JsonClassDiscriminator("type")
sealed interface UserEvent {
    @Serializable
    @SerialName("run")
    data class Run(
        @SerialName("runId") val runId: String,
        val state: String? = null,
    ) : UserEvent

    @Serializable
    @SerialName("notification")
    data class Notification(
        @SerialName("notificationId") val notificationId: String,
    ) : UserEvent

    @Serializable
    @SerialName("brief")
    data class Brief(
        @SerialName("briefId") val briefId: String,
        val seq: Long = 0,
    ) : UserEvent

    @Serializable
    @SerialName("channel")
    data class Channel(
        @SerialName("channelId") val channelId: String,
    ) : UserEvent

    @Serializable
    @SerialName("conversation")
    data class Conversation(
        @SerialName("conversationId") val conversationId: String,
    ) : UserEvent

    /** Ephemeral presence. Never stored, never re-read — and a lost
     *  `typing: false` costs a few seconds of dots, so clients expire these
     *  themselves rather than trusting the stream to close them. */
    @Serializable
    @SerialName("typing")
    data class Typing(
        @SerialName("channelId") val channelId: String,
        @SerialName("userId") val userId: String,
        val typing: Boolean = false,
    ) : UserEvent
}

const val ME_EVENTS_PATH = "/api/me/events"

private val eventJson =
    Json {
        ignoreUnknownKeys = true
        classDiscriminator = "type"
        // A `type` this build has never heard of is the api being newer than
        // the app, which is normal and must not kill the stream.
        isLenient = true
    }

/** Decode one frame's data. Null when it is not a UserEvent this build knows —
 *  a newer api's event type, or a payload that is not one at all. */
fun decodeUserEvent(data: String): UserEvent? =
    try {
        eventJson.decodeFromString<UserEvent>(data)
    } catch (_: Throwable) {
        null
    }

/**
 * The live stream, as a cold Flow.
 *
 * The timeout is disabled for this one request and that is not an oversight:
 * [talariaHttpClient] sets a deliberately short request timeout so a failing
 * call fails while the phone is still in someone's hand, and an SSE stream is
 * the one request that is *supposed* to stay open for hours. A ping arrives
 * every 25 seconds, so silence much longer than that is a dead connection
 * rather than a quiet one.
 *
 * Reconnection is the COLLECTOR's business, not this function's. A Flow that
 * retried internally would hide the one thing a caller needs to know — that it
 * dropped — and on a phone that information is load-bearing, because the right
 * answer after a reconnect is to re-read state that changed while nobody was
 * listening.
 */
fun userEvents(
    client: HttpClient,
    instance: Instance,
    credential: Credential,
): Flow<UserEvent> =
    flow {
        client
            .prepareGet("${instance.origin}$ME_EVENTS_PATH") {
                credential.applyTo(this)
                header("Accept", "text/event-stream")
                // Proxies that buffer would defeat the whole point.
                header("Cache-Control", "no-cache")
                timeout { requestTimeoutMillis = HttpTimeoutConfig.INFINITE_TIMEOUT_MS }
            }.execute { response ->
                val channel = response.bodyAsChannel()
                val parser = SseParser()
                while (true) {
                    val line = channel.readUTF8Line() ?: break
                    // readUTF8Line strips the terminator, so the blank line
                    // that dispatches a frame has to be put back.
                    for (frame in parser.feed(line + "\n")) {
                        decodeUserEvent(frame.data)?.let { emit(it) }
                    }
                }
            }
    }
