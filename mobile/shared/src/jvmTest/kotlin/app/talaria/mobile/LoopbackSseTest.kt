package app.talaria.mobile

import app.talaria.mobile.account.Credential
import app.talaria.mobile.account.Instance
import app.talaria.mobile.net.ME_EVENTS_PATH
import app.talaria.mobile.net.UserEvent
import app.talaria.mobile.net.userEvents
import com.sun.net.httpserver.HttpServer
import io.ktor.client.HttpClient
import io.ktor.client.engine.cio.CIO
import io.ktor.client.plugins.HttpTimeout
import io.ktor.client.plugins.contentnegotiation.ContentNegotiation
import io.ktor.serialization.kotlinx.json.json
import kotlinx.coroutines.flow.take
import kotlinx.coroutines.flow.toList
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.withTimeout
import kotlinx.serialization.json.Json
import java.net.InetSocketAddress
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import kotlin.test.AfterTest
import kotlin.test.BeforeTest
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertTrue

/**
 * The firehose, streamed over a real socket.
 *
 * `SseTest` proves the parser against strings; this proves the whole path —
 * request headers, a chunked response that never ends, reading it line by line
 * as it arrives, and decoding events out of it — against a server that writes
 * frames slowly and out of step with packet boundaries, which is how a real one
 * behaves.
 *
 * It also pins the two things a phone depends on and a string test cannot see:
 * that the request carries the account's own credential, and that the client's
 * short request timeout is DISABLED for this one call. Without the latter, a
 * stream that is behaving perfectly dies after twenty seconds.
 */
class LoopbackSseTest {
    private lateinit var server: HttpServer
    private lateinit var origin: String
    private var sentCookie: String? = null
    private var sentAccept: String? = null
    private val connected = CountDownLatch(1)

    @BeforeTest
    fun start() {
        server = HttpServer.create(InetSocketAddress("127.0.0.1", 0), 0)
        server.createContext(ME_EVENTS_PATH) { ex ->
            sentCookie = ex.requestHeaders.getFirst("Cookie")
            sentAccept = ex.requestHeaders.getFirst("Accept")
            ex.responseHeaders.add("Content-Type", "text/event-stream")
            ex.responseHeaders.add("Cache-Control", "no-cache")
            // 0 = chunked, no content length: the stream stays open.
            ex.sendResponseHeaders(200, 0)
            connected.countDown()
            val out = ex.responseBody
            fun write(s: String) {
                out.write(s.encodeToByteArray())
                out.flush()
            }
            // Deliberately ragged, the way a real stream arrives: a preamble,
            // a frame in two writes, a keep-alive comment between events.
            write(": connected\n\n")
            write("""data: {"type":"notification","notif""")
            write("""icationId":"n1"}""" + "\n\n")
            write(": ping\n\n")
            write("""data: {"type":"channel","channelId":"c1"}""" + "\n\n")
            write(""" data: {"type":"brief","briefId":"b1","seq":3}""".trimStart() + "\n\n")
            // Left open on purpose — the collector stops, not the server.
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
            // The same deliberately-short timeout the app's real client sets,
            // so this test would catch a stream that forgot to opt out of it.
            install(HttpTimeout) {
                requestTimeoutMillis = 2_000
                connectTimeoutMillis = 2_000
            }
        }

    private fun instance() = Instance(id = "i", origin = origin, label = "Outcrop Labs")

    @Test
    fun events_arrive_decoded_from_a_live_stream(): Unit =
        runBlocking {
            client().use { client ->
                val events =
                    withTimeout(20_000) {
                        userEvents(client, instance(), Credential.Session("sid-jon")).take(3).toList()
                    }

                // The preamble and the ping are absent; the frame split across
                // two writes arrived whole.
                assertEquals(
                    listOf(
                        UserEvent.Notification("n1"),
                        UserEvent.Channel("c1"),
                        UserEvent.Brief(briefId = "b1", seq = 3),
                    ),
                    events,
                )
            }
        }

    @Test
    fun the_stream_carries_the_accounts_own_credential_and_asks_for_sse(): Unit =
        runBlocking {
            client().use { client ->
                withTimeout(20_000) {
                    userEvents(client, instance(), Credential.Session("sid-sam")).take(1).toList()
                }
                assertTrue(connected.await(5, TimeUnit.SECONDS))
                assertEquals("${Credential.COOKIE_NAME}=sid-sam", sentCookie)
                assertTrue(
                    sentAccept?.contains("text/event-stream") == true,
                    "a stream request must ask for one: $sentAccept",
                )
            }
        }

    /**
     * The regression this file exists for. The app's client sets a short
     * request timeout so a failing call fails fast; a stream must opt out, or
     * a perfectly healthy connection dies on a timer. The server writes three
     * frames and then goes quiet for longer than that timeout — if the opt-out
     * regressed, collecting would throw instead of simply waiting.
     */
    @Test
    fun a_quiet_stream_is_not_killed_by_the_request_timeout(): Unit =
        runBlocking {
            client().use { client ->
                val events =
                    withTimeout(20_000) {
                        // The fourth event never comes; take(3) completes only
                        // because the first three did, after the server has
                        // been silent well past the 2s request timeout.
                        userEvents(client, instance(), Credential.Session("sid-jon")).take(3).toList()
                    }
                assertEquals(3, events.size)
            }
        }
}
