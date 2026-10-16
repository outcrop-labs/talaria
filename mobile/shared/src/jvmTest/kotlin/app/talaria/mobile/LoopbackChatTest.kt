package app.talaria.mobile

import app.talaria.mobile.account.Credential
import app.talaria.mobile.account.Instance
import app.talaria.mobile.net.CHAT_PATH
import app.talaria.mobile.net.ChatEvent
import app.talaria.mobile.net.ToolStatus
import app.talaria.mobile.net.chatStream
import com.sun.net.httpserver.HttpServer
import io.ktor.client.HttpClient
import io.ktor.client.engine.cio.CIO
import io.ktor.client.plugins.HttpTimeout
import io.ktor.client.plugins.contentnegotiation.ContentNegotiation
import io.ktor.serialization.kotlinx.json.json
import kotlinx.coroutines.flow.toList
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.withTimeout
import kotlinx.serialization.json.Json
import java.net.InetSocketAddress
import kotlin.test.AfterTest
import kotlin.test.BeforeTest
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertTrue

/**
 * The chat stream over a real socket.
 *
 * `ChatParseTest` proves the frame vocabulary against strings. This proves the
 * whole path: the request body, the identity headers that arrive before any
 * token, an OpenAI-shaped stream written raggedly, named tool-progress events
 * interleaved with deltas, and `[DONE]`.
 *
 * It also pins the regression that would be hardest to notice: the body is read
 * INSIDE Ktor's `execute` block, because a response body does not outlive it.
 * An earlier draft of `chatStream` drained the stream into a list and replayed
 * it, which passed every string test and silently destroyed the token-by-token
 * behaviour the screen exists to show.
 */
class LoopbackChatTest {
    private lateinit var server: HttpServer
    private lateinit var origin: String
    private var sentBody: String? = null
    private var sentCookie: String? = null

    @BeforeTest
    fun start() {
        server = HttpServer.create(InetSocketAddress("127.0.0.1", 0), 0)
        server.createContext(CHAT_PATH) { ex ->
            sentBody = ex.requestBody.readBytes().decodeToString()
            sentCookie = ex.requestHeaders.getFirst("Cookie")
            ex.responseHeaders.add("Content-Type", "text/event-stream")
            ex.responseHeaders.add("X-Conversation-Id", "conv-live-1")
            ex.responseHeaders.add("X-Message-Id", "msg-live-1")
            ex.sendResponseHeaders(200, 0)
            val out = ex.responseBody
            fun write(s: String) {
                out.write(s.encodeToByteArray())
                out.flush()
            }
            // A realistic turn: a tool starts, deltas arrive split across
            // writes, the tool finishes with no label, usage lands, then DONE.
            write("""event: hermes.tool.progress
data: {"tool":"bash","emoji":"🔧","label":"running tests","toolCallId":"tc1","status":"running"}

""")
            write("""data: {"choices":[{"delta":{"content":"All "}}]}""" + "\n\n")
            write("""data: {"choices":[{"delta":{"content":"gr""")
            write("""een."}}]}""" + "\n\n")
            write("""event: hermes.tool.progress
data: {"tool":"bash","toolCallId":"tc1","status":"completed"}

""")
            write("""data: {"choices":[{"delta":{}}],"usage":{"prompt_tokens":120,"completion_tokens":45}}""" + "\n\n")
            write("data: [DONE]\n\n")
            out.close()
        }
        server.start()
        origin = "http://127.0.0.1:${server.address.port}"
    }

    @AfterTest
    fun stop(): Unit = server.stop(0)

    private fun client() =
        HttpClient(CIO) {
            expectSuccess = false
            install(ContentNegotiation) { json(Json { ignoreUnknownKeys = true }) }
            // The app's deliberately-short timeout: a stream must opt out of it
            // or a long answer dies mid-sentence.
            install(HttpTimeout) {
                requestTimeoutMillis = 2_000
                connectTimeoutMillis = 2_000
            }
        }

    private fun instance() = Instance(id = "i", origin = origin, label = "Outcrop Labs")

    @Test
    fun a_whole_turn_streams_and_decodes(): Unit =
        runBlocking {
            client().use { client ->
                val events =
                    withTimeout(20_000) {
                        chatStream(
                            client,
                            instance(),
                            Credential.Session("sid-jon"),
                            model = "claude-opus-5",
                            content = "run the tests",
                        ).toList()
                    }

                // Identity first, before any token.
                assertEquals(
                    ChatEvent.Started(conversationId = "conv-live-1", messageId = "msg-live-1"),
                    events.first(),
                )

                // The delta split across two writes arrived as two content
                // events whose concatenation is the answer — which is exactly
                // what the screen appends.
                val answer = events.filterIsInstance<ChatEvent.Content>().joinToString("") { it.text }
                assertEquals("All green.", answer)

                // Both tool frames, in order, and the completion carries no
                // label — the merge rule's whole reason for existing.
                val tools = events.filterIsInstance<ChatEvent.Tool>()
                assertEquals(2, tools.size)
                assertEquals(ToolStatus.Running, tools[0].status)
                assertEquals("🔧 running tests", tools[0].label)
                assertEquals(ToolStatus.Completed, tools[1].status)
                assertEquals("", tools[1].label)

                assertEquals(ChatEvent.Usage(120, 45), events.filterIsInstance<ChatEvent.Usage>().single())
                // `[DONE]` is a terminator, not an event.
                assertTrue(events.none { it is ChatEvent.Queued })
            }
        }

    @Test
    fun the_request_carries_the_model_the_text_and_the_credential(): Unit =
        runBlocking {
            client().use { client ->
                withTimeout(20_000) {
                    chatStream(
                        client,
                        instance(),
                        Credential.Session("sid-sam"),
                        model = "claude-opus-5",
                        content = "run the tests",
                        conversationId = "conv-prior",
                    ).toList()
                }
                val body = sentBody ?: ""
                assertTrue(body.contains("\"model\":\"claude-opus-5\""), body)
                assertTrue(body.contains("\"content\":\"run the tests\""), body)
                assertTrue(body.contains("\"conversationId\":\"conv-prior\""), body)
                assertEquals("${Credential.COOKIE_NAME}=sid-sam", sentCookie)
            }
        }

    /** A refused turn is an event, not an exception — the UI already switches
     *  on this stream's vocabulary and should not need a second channel. */
    @Test
    fun a_server_error_arrives_as_a_failed_event(): Unit =
        runBlocking {
            val broken = HttpServer.create(InetSocketAddress("127.0.0.1", 0), 0)
            broken.createContext(CHAT_PATH) { ex -> ex.sendResponseHeaders(503, -1); ex.close() }
            broken.start()
            try {
                client().use { client ->
                    val events =
                        withTimeout(20_000) {
                            chatStream(
                                client,
                                Instance(id = "i", origin = "http://127.0.0.1:${broken.address.port}", label = "X"),
                                Credential.Session("sid"),
                                model = "m",
                                content = "hi",
                            ).toList()
                        }
                    val failed = events.filterIsInstance<ChatEvent.Failed>().single()
                    assertTrue(failed.reason.contains("503"), failed.reason)
                }
            } finally {
                broken.stop(0)
            }
        }

    @Test
    fun an_unauthorized_turn_arrives_as_expired(): Unit =
        runBlocking {
            val locked = HttpServer.create(InetSocketAddress("127.0.0.1", 0), 0)
            locked.createContext(CHAT_PATH) { ex -> ex.sendResponseHeaders(401, -1); ex.close() }
            locked.start()
            try {
                client().use { client ->
                    val events =
                        withTimeout(20_000) {
                            chatStream(
                                client,
                                Instance(id = "i", origin = "http://127.0.0.1:${locked.address.port}", label = "X"),
                                Credential.Session("stale"),
                                model = "m",
                                content = "hi",
                            ).toList()
                        }
                    assertEquals(listOf(ChatEvent.Expired), events)
                }
            } finally {
                locked.stop(0)
            }
        }
}
