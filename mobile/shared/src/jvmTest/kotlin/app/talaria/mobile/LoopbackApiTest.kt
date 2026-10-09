package app.talaria.mobile

import app.talaria.mobile.account.Credential
import app.talaria.mobile.account.Instance
import app.talaria.mobile.account.MeResult
import app.talaria.mobile.account.SignInResult
import app.talaria.mobile.account.readMe
import app.talaria.mobile.account.signIn
import app.talaria.mobile.instance.ProbeResult
import app.talaria.mobile.instance.probeInstance
import com.sun.net.httpserver.HttpExchange
import com.sun.net.httpserver.HttpServer
import io.ktor.client.HttpClient
import io.ktor.client.engine.cio.CIO
import io.ktor.client.plugins.contentnegotiation.ContentNegotiation
import io.ktor.serialization.kotlinx.json.json
import kotlinx.coroutines.runBlocking
import kotlinx.serialization.json.Json
import java.net.InetSocketAddress
import kotlin.test.AfterTest
import kotlin.test.BeforeTest
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertIs
import kotlin.test.assertTrue

/**
 * The same client code, over a real socket.
 *
 * WHY THIS EXISTS BESIDE THE MOCKENGINE TESTS. MockEngine short-circuits the
 * transport: it hands a response object straight back, so it can prove what the
 * code *intends* and nothing about whether it works. Everything riskiest in the
 * auth path lives in the part MockEngine skips — building the URL, writing the
 * Cookie header on the wire, and parsing a real multi-attribute `Set-Cookie`
 * (`HttpOnly; SameSite=Lax; Max-Age=604800`) out of a real response. A JDK
 * `HttpServer` on an ephemeral port plus Ktor's CIO engine exercises all of it
 * for the price of one test file and no new infrastructure.
 *
 * It is NOT a substitute for the real api. The devbox image carries no cargo,
 * so a live Talaria api cannot run in-box; the responses below are shaped by
 * hand from `docs/api/account.md` and the handlers' own doc comments. What this
 * proves is that the client speaks HTTP correctly — not that the api agrees.
 * docs/MOBILE.md records the gap.
 */
class LoopbackApiTest {
    private lateinit var server: HttpServer
    private lateinit var origin: String

    /** The cookie string the api actually writes, attribute for attribute —
     *  `talaria-session/src/lib.rs` builds exactly this shape. */
    private val setCookie = "${Credential.COOKIE_NAME}=sid-abc123; Path=/; HttpOnly; SameSite=Lax; Max-Age=604800"

    /** Cookie headers the last authed request arrived with. */
    private val cookiesSeen = mutableListOf<String?>()

    private fun HttpExchange.reply(
        status: Int,
        body: String,
        contentType: String = "application/json",
    ) {
        responseHeaders.add("Content-Type", contentType)
        val bytes = body.encodeToByteArray()
        sendResponseHeaders(status, bytes.size.toLong())
        responseBody.use { it.write(bytes) }
    }

    @BeforeTest
    fun start() {
        server = HttpServer.create(InetSocketAddress("127.0.0.1", 0), 0)
        server.createContext("/api/well-known/talaria-instance") { ex ->
            ex.reply(200, """{"instance":"3f2504e0-4f89-11d3-9a0c-0305e82c3301","companyName":"Outcrop Labs"}""")
        }
        server.createContext("/api/auth/password") { ex ->
            val body = ex.requestBody.readBytes().decodeToString()
            if (body.contains("\"password\":\"correct-horse\"")) {
                ex.responseHeaders.add("Set-Cookie", setCookie)
                ex.reply(200, """{"ok":true}""")
            } else {
                ex.reply(401, """{"error":"bad credentials"}""")
            }
        }
        server.createContext("/api/me") { ex ->
            cookiesSeen += ex.requestHeaders.getFirst("Cookie")
            val cookie = ex.requestHeaders.getFirst("Cookie")
            if (cookie == "${Credential.COOKIE_NAME}=sid-abc123") {
                ex.reply(200, """{"preferredModel":"claude-opus-5","timezone":"Europe/London","title":"Founder"}""")
            } else {
                ex.reply(401, """{"error":"no session"}""")
            }
        }
        server.start()
        origin = "http://127.0.0.1:${server.address.port}"
    }

    @AfterTest
    fun stop(): Unit = server.stop(0)

    private fun realClient() =
        HttpClient(CIO) {
            expectSuccess = false
            install(ContentNegotiation) { json(Json { ignoreUnknownKeys = true; explicitNulls = false }) }
        }

    private fun instance() = Instance(id = "3f2504e0-4f89-11d3-9a0c-0305e82c3301", origin = origin, label = "Outcrop Labs")

    @Test
    fun the_beacon_probe_works_over_a_socket(): Unit =
        runBlocking {
            realClient().use { client ->
                val result = probeInstance(client, origin)
                assertIs<ProbeResult.Found>(result)
                assertEquals("Outcrop Labs", result.label)
            }
        }

    /** The whole point of the file: a real `Set-Cookie` with real attributes,
     *  parsed into a credential, then replayed as a real `Cookie` header that a
     *  real server accepts. */
    @Test
    fun signing_in_captures_the_session_and_replays_it(): Unit =
        runBlocking {
            realClient().use { client ->
                val signedIn = signIn(client, instance(), "jon@outcroplabs.com", "correct-horse")
                assertIs<SignInResult.Ok>(signedIn)
                assertEquals(Credential.Session("sid-abc123"), signedIn.credential)

                val me = readMe(client, instance(), signedIn.credential)
                assertIs<MeResult.Ok>(me)
                assertEquals("Europe/London", me.me.timezone)
                assertEquals("Founder", me.me.title)

                // The credential went out as the api's own cookie name — a
                // header the server read, not something the client kept to
                // itself.
                assertEquals("${Credential.COOKIE_NAME}=sid-abc123", cookiesSeen.last())
            }
        }

    @Test
    fun a_wrong_password_is_refused_in_words_a_person_can_act_on(): Unit =
        runBlocking {
            realClient().use { client ->
                val result = signIn(client, instance(), "jon@outcroplabs.com", "nope")
                assertIs<SignInResult.Refused>(result)
                assertTrue(result.reason.contains("did not match"), result.reason)
            }
        }

    /** No ambient cookie jar: the client must not remember a session from one
     *  request and volunteer it on the next. That is the isolation a phone has
     *  to provide explicitly, because it has one process for every account. */
    @Test
    fun the_client_never_replays_a_session_it_was_not_given(): Unit =
        runBlocking {
            realClient().use { client ->
                val signedIn = signIn(client, instance(), "jon@outcroplabs.com", "correct-horse")
                assertIs<SignInResult.Ok>(signedIn)

                // A second account's request, carrying a DIFFERENT credential.
                val other = readMe(client, instance(), Credential.Session("someone-elses-sid"))
                assertIs<MeResult.Expired>(other)
                assertEquals("${Credential.COOKIE_NAME}=someone-elses-sid", cookiesSeen.last())
            }
        }

    @Test
    fun an_expired_session_reads_as_expired_rather_than_an_error(): Unit =
        runBlocking {
            realClient().use { client ->
                assertIs<MeResult.Expired>(readMe(client, instance(), Credential.Session("stale")))
            }
        }

    @Test
    fun an_origin_with_nothing_listening_fails_with_a_reachability_reason(): Unit =
        runBlocking {
            realClient().use { client ->
                // Port 1 on loopback: reserved, and nothing will ever answer.
                val result = probeInstance(client, "http://127.0.0.1:1")
                assertIs<ProbeResult.Failed>(result)
                assertTrue(result.reason.contains("could not reach"), result.reason)
            }
        }
}
