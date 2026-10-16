package app.talaria.mobile.net

import app.talaria.mobile.account.Credential
import app.talaria.mobile.account.Instance
import io.ktor.client.HttpClient
import io.ktor.client.plugins.HttpTimeoutConfig
import io.ktor.client.plugins.timeout
import io.ktor.client.request.header
import io.ktor.client.request.preparePost
import io.ktor.client.request.setBody
import io.ktor.client.statement.bodyAsChannel
import io.ktor.http.ContentType
import io.ktor.http.HttpStatusCode
import io.ktor.http.contentType
import io.ktor.utils.io.readLine
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.flow
import kotlinx.serialization.SerialName
import kotlinx.serialization.Serializable
import kotlinx.serialization.json.Json

/**
 * `POST /api/chat` — the controller's command line: tell an agent to do
 * something and watch the answer arrive.
 *
 * This file is the Kotlin half of [`ui/src/lib/sse-parse.ts`][1], and that is
 * deliberate rather than incidental: the scope doc named that module as one of
 * the genuinely portable thousand lines, and here it is, ported. The frame
 * vocabulary is the web client's, case for case, because both read the same
 * stream — and if they ever disagree, one of them renders a reply wrong.
 *
 * [1]: ../../../../../../../../ui/src/lib/sse-parse.ts
 *
 * THE STREAM IS TEED SERVER-SIDE. `/api/chat` persists the turn to Postgres on
 * one branch and streams to the client on the other, so the server owns history
 * and a reply survives a reload mid-stream. A phone benefits more than a laptop
 * does: a radio drops, and the answer is still there.
 *
 * Three frame shapes arrive, and the discrimination order matters:
 *  1. `data: [DONE]` — the end, and NOT json. Parsing it would throw.
 *  2. `event: hermes.tool.progress` / `claude.tool.progress` — an agent
 *     narrating its own tool use. Named events, matched on the event name
 *     BEFORE any json parse.
 *  3. anything else — an OpenAI-shaped chunk: text at
 *     `choices[0].delta.content`, reasoning at `delta.reasoning` or
 *     `delta.reasoning_content`, and a final chunk may carry `usage`.
 */
sealed interface ChatEvent {
    /**
     * The turn's identity, first. It rides in `X-Conversation-Id` /
     * `X-Message-Id` headers, so it is known before a single token arrives —
     * and it has to reach the caller somehow, because the next turn needs the
     * conversation id to continue the same thread. An event rather than a
     * return value is what keeps this one cold Flow instead of two things to
     * hold in step.
     */
    data class Started(
        val conversationId: String?,
        val messageId: String?,
    ) : ChatEvent

    data class Content(val text: String) : ChatEvent

    /** The model thinking out loud, shown apart from the answer. */
    data class Reasoning(val text: String) : ChatEvent

    data class Tool(
        val id: String? = null,
        val name: String,
        val label: String,
        val status: ToolStatus? = null,
    ) : ChatEvent

    data class Usage(
        val promptTokens: Int,
        val completionTokens: Int,
    ) : ChatEvent

    /**
     * The server answered with JSON instead of a stream (202) because the reply
     * joined history rather than streaming. Not produced by frame parsing.
     * The web client learned to keep this in the union the hard way: leaving it
     * out made the stream's inferred type wider than its declared one.
     */
    data object Queued : ChatEvent

    data class Failed(val reason: String) : ChatEvent

    /** The credential is no longer good; the account should be signed out. */
    data object Expired : ChatEvent
}

enum class ToolStatus { Running, Completed }

data class ToolCall(
    val id: String? = null,
    val name: String,
    val label: String,
    val status: ToolStatus,
)

const val CHAT_PATH = "/api/chat"

private val chatJson = Json { ignoreUnknownKeys = true; isLenient = true }

private const val TOOL_PROGRESS_HERMES = "hermes.tool.progress"
private const val TOOL_PROGRESS_CLAUDE = "claude.tool.progress"

@Serializable
private data class Chunk(
    val choices: List<Choice> = emptyList(),
    val usage: UsageWire? = null,
)

@Serializable
private data class Choice(val delta: Delta? = null)

@Serializable
private data class Delta(
    val content: String? = null,
    val reasoning: String? = null,
    @SerialName("reasoning_content") val reasoningContent: String? = null,
)

@Serializable
private data class UsageWire(
    @SerialName("prompt_tokens") val promptTokens: Int = 0,
    @SerialName("completion_tokens") val completionTokens: Int = 0,
)

@Serializable
private data class ToolProgress(
    val tool: String? = null,
    val name: String? = null,
    val emoji: String? = null,
    val label: String? = null,
    @SerialName("toolCallId") val toolCallId: String? = null,
    @SerialName("tool_call_id") val toolCallIdSnake: String? = null,
    val status: String? = null,
)

/**
 * One frame → zero or more events.
 *
 * A list, because one OpenAI chunk can legitimately carry both a delta and the
 * final `usage`, and dropping the second would lose the only cost signal the
 * stream gives.
 */
fun parseChatFrame(
    event: String?,
    data: String,
): List<ChatEvent> {
    if (data.isEmpty() || data == "[DONE]") return emptyList()

    if (event == TOOL_PROGRESS_HERMES || event == TOOL_PROGRESS_CLAUDE) {
        return parseToolProgress(data)?.let { listOf(it) } ?: emptyList()
    }

    return try {
        val chunk = chatJson.decodeFromString<Chunk>(data)
        val out = mutableListOf<ChatEvent>()
        val delta = chunk.choices.firstOrNull()?.delta
        val reasoning = delta?.reasoning?.takeIf { it.isNotEmpty() } ?: delta?.reasoningContent
        val text = delta?.content
        when {
            !text.isNullOrEmpty() -> out += ChatEvent.Content(text)
            !reasoning.isNullOrEmpty() -> out += ChatEvent.Reasoning(reasoning)
        }
        chunk.usage?.let { usage ->
            if (usage.promptTokens != 0 || usage.completionTokens != 0) {
                out += ChatEvent.Usage(usage.promptTokens, usage.completionTokens)
            }
        }
        out
    } catch (_: Throwable) {
        // A keep-alive or a partial frame. Not an error — the stream carries
        // both by design, and throwing would end a healthy reply.
        emptyList()
    }
}

private fun parseToolProgress(payload: String): ChatEvent.Tool? =
    try {
        val raw = chatJson.decodeFromString<ToolProgress>(payload)
        val name = raw.tool?.takeIf { it.isNotEmpty() } ?: raw.name?.takeIf { it.isNotEmpty() } ?: "tool"
        val label = listOfNotNull(raw.emoji, raw.label).filter { it.isNotEmpty() }.joinToString(" ").trim()
        val id = raw.toolCallId?.takeIf { it.isNotEmpty() } ?: raw.toolCallIdSnake?.takeIf { it.isNotEmpty() }
        val status =
            when (raw.status?.lowercase()) {
                "running" -> ToolStatus.Running
                "completed", "complete" -> ToolStatus.Completed
                else -> null
            }
        // A frame with neither a label nor an id says nothing renderable.
        if (label.isEmpty() && id == null) null else ChatEvent.Tool(id, name, label, status)
    } catch (_: Throwable) {
        null
    }

/**
 * Fold a tool event into a running list: dedupe by id, else by name+running.
 *
 * The empty-label rule is load-bearing and easy to get wrong — a later
 * "completed" frame carries NO label, so merging must not overwrite a good
 * label with an empty one, or a finished tool loses its name mid-flight.
 */
fun mergeTool(
    tools: List<ToolCall>,
    event: ChatEvent.Tool,
): List<ToolCall> {
    val index =
        if (event.id != null) {
            tools.indexOfFirst { it.id == event.id }
        } else {
            tools.indexOfFirst { it.name == event.name && it.status == ToolStatus.Running }
        }
    if (index < 0) {
        return tools +
            ToolCall(
                id = event.id,
                name = event.name,
                label = event.label,
                status = event.status ?: ToolStatus.Running,
            )
    }
    val existing = tools[index]
    return tools.toMutableList().also {
        it[index] =
            existing.copy(
                label = event.label.takeIf { l -> l.isNotEmpty() } ?: existing.label,
                status = event.status ?: existing.status,
            )
    }
}

@Serializable
private data class ChatBody(
    val model: String,
    val content: String,
    @SerialName("conversationId") val conversationId: String? = null,
    val kind: String? = null,
)

/**
 * Send a turn; get the reply token by token.
 *
 * ONE COLD FLOW, and the request lives INSIDE it. That is not a style choice:
 * Ktor's response body does not outlive its `execute` block, so building the
 * request outside and handing back a lazy Flow would read from a closed
 * channel. Everything — the request, the headers, the frames — happens while
 * somebody is collecting.
 *
 * Failures are events, not exceptions, because this stream already has a typed
 * vocabulary the UI switches on; a thrown error would need a second channel for
 * the same information. The request timeout is disabled for the same reason the
 * firehose disables it: the app's client fails fast on purpose, and a model
 * composing a long answer is the one request meant to take its time.
 */
fun chatStream(
    client: HttpClient,
    instance: Instance,
    credential: Credential,
    model: String,
    content: String,
    conversationId: String? = null,
): Flow<ChatEvent> =
    flow {
        try {
            client
                .preparePost("${instance.origin}$CHAT_PATH") {
                    credential.applyTo(this)
                    contentType(ContentType.Application.Json)
                    header("Accept", "text/event-stream")
                    header("Cache-Control", "no-cache")
                    timeout { requestTimeoutMillis = HttpTimeoutConfig.INFINITE_TIMEOUT_MS }
                    setBody(
                        ChatBody(
                            model = model,
                            content = content,
                            conversationId = conversationId,
                            kind = "chat",
                        ),
                    )
                }.execute { response ->
                    when {
                        response.status == HttpStatusCode.Unauthorized ||
                            response.status == HttpStatusCode.Forbidden -> {
                            emit(ChatEvent.Expired)
                            return@execute
                        }
                        response.status.value !in 200..299 -> {
                            emit(ChatEvent.Failed("${instance.label} answered ${response.status.value}"))
                            return@execute
                        }
                    }

                    emit(
                        ChatEvent.Started(
                            conversationId = response.headers["X-Conversation-Id"] ?: conversationId,
                            messageId = response.headers["X-Message-Id"],
                        ),
                    )

                    // A 202 means the reply joined history instead of
                    // streaming. There are no frames to read, and trying would
                    // wait for a body that never comes.
                    if (response.status == HttpStatusCode.Accepted) {
                        emit(ChatEvent.Queued)
                        return@execute
                    }

                    val channel = response.bodyAsChannel()
                    val parser = SseParser()
                    while (true) {
                        val line = channel.readLine() ?: break
                        for (frame in parser.feed(line + "\n")) {
                            parseChatFrame(frame.event, frame.data).forEach { emit(it) }
                        }
                    }
                }
        } catch (e: Throwable) {
            emit(ChatEvent.Failed("the reply stream failed: ${e.message ?: "unknown"}"))
        }
    }
