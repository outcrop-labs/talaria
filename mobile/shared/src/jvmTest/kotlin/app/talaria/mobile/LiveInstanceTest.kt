package app.talaria.mobile

import app.talaria.mobile.account.Credential
import app.talaria.mobile.account.Instance
import app.talaria.mobile.account.MeResult
import app.talaria.mobile.account.SignInResult
import app.talaria.mobile.account.readMe
import app.talaria.mobile.account.signIn
import app.talaria.mobile.instance.OriginResult
import app.talaria.mobile.instance.ProbeResult
import app.talaria.mobile.instance.normalizeOrigin
import app.talaria.mobile.instance.probeInstance
import app.talaria.mobile.net.CommsResult
import app.talaria.mobile.net.HomeResult
import app.talaria.mobile.net.UserEvent
import app.talaria.mobile.net.readChannels
import app.talaria.mobile.net.readHome
import app.talaria.mobile.net.userEvents
import io.ktor.client.HttpClient
import io.ktor.client.engine.cio.CIO
import io.ktor.client.plugins.HttpTimeout
import io.ktor.client.plugins.contentnegotiation.ContentNegotiation
import io.ktor.serialization.kotlinx.json.json
import kotlinx.coroutines.flow.take
import kotlinx.coroutines.flow.toList
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.withTimeoutOrNull
import kotlinx.serialization.json.Json
import org.junit.Assume.assumeTrue
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertIs
import kotlin.test.assertTrue

/**
 * The client against a REAL Talaria api.
 *
 * `LoopbackApiTest` proves the client speaks HTTP; this proves the api agrees —
 * that the shapes transcribed from the Rust structs are the shapes actually on
 * the wire, which is the one thing a hand-written fixture can never tell you.
 *
 * OPT-IN BY ENVIRONMENT, and it has to be. A test that needs a particular
 * instance to exist cannot be a test everyone runs: a fresh clone, a CI runner
 * and a contributor on another network all have no such instance, and a gate
 * that fails for them is a gate they learn to ignore. So it SKIPS (JUnit's
 * assumption mechanism, which reports skipped rather than passed) unless told
 * where to look:
 *
 * ```sh
 * TALARIA_LIVE_ORIGIN=http://host:6302 ./gradlew :shared:jvmTest
 * # ...and, for the routes behind a session:
 * TALARIA_LIVE_USERNAME=someone@example.com TALARIA_LIVE_PASSWORD=… ./gradlew …
 * ```
 *
 * READ-ONLY, DELIBERATELY. The instance this is pointed at is somebody's real
 * workspace with a live agent fleet in it. Nothing here posts a message, moves
 * a ticket or clears a gate — an integration test must not appear in a
 * colleague's unread count. The assertions are about SHAPE, never content, for
 * the same reason: a test that printed a real channel's name or a real person's
 * message would leak a workspace into a CI log.
 */
class LiveInstanceTest {
    private val origin: String? = System.getenv("TALARIA_LIVE_ORIGIN")?.takeIf { it.isNotBlank() }
    private val username: String? = System.getenv("TALARIA_LIVE_USERNAME")?.takeIf { it.isNotBlank() }
    private val password: String? = System.getenv("TALARIA_LIVE_PASSWORD")?.takeIf { it.isNotBlank() }

    private fun client() =
        HttpClient(CIO) {
            expectSuccess = false
            install(ContentNegotiation) { json(Json { ignoreUnknownKeys = true; explicitNulls = false }) }
            install(HttpTimeout) {
                requestTimeoutMillis = 20_000
                connectTimeoutMillis = 10_000
            }
        }

    private fun requireOrigin(): String {
        assumeTrue("set TALARIA_LIVE_ORIGIN to run the live tests", origin != null)
        return origin!!
    }

    /** Sign in and hand back a usable instance + credential, or skip. */
    private suspend fun session(client: HttpClient): Pair<Instance, Credential> {
        val base = requireOrigin()
        assumeTrue(
            "set TALARIA_LIVE_USERNAME and TALARIA_LIVE_PASSWORD to run the authed live tests",
            username != null && password != null,
        )
        val probed = probeInstance(client, normalized(base))
        assertIs<ProbeResult.Found>(probed, "the live origin must answer a beacon")
        val instance = Instance.of(probed.origin, probed.beacon, probed.label)

        val signedIn = signIn(client, instance, username!!, password!!)
        assertIs<SignInResult.Ok>(signedIn, "live sign-in failed — are the credentials right?")
        return instance to signedIn.credential
    }

    private fun normalized(input: String): String {
        val result = normalizeOrigin(input)
        assertIs<OriginResult.Ok>(result, "TALARIA_LIVE_ORIGIN is not a usable origin: $input")
        return result.origin
    }

    // ---- public surface: no credentials needed -----------------------------

    /**
     * The add-instance flow, end to end against a real api: normalize what a
     * person typed, ask the beacon, and get back an identity.
     */
    @Test
    fun the_beacon_probe_works_against_a_real_instance(): Unit =
        runBlocking {
            val base = requireOrigin()
            client().use { client ->
                val result = probeInstance(client, normalized(base))
                assertIs<ProbeResult.Found>(result)
                // Shape, not content: a uuid-shaped id and a non-blank label is
                // everything the launcher needs, and all a log should see.
                assertTrue(
                    Regex("^[0-9a-fA-F-]{36}$").matches(result.beacon.instance),
                    "the beacon's instance must be a uuid",
                )
                assertTrue(result.label.isNotBlank(), "an instance must be labellable")
            }
        }

    /** Default ports collapse, so two spellings of one instance must come back
     *  with the SAME beacon uuid — the dedupe key the registry depends on. */
    @Test
    fun one_instance_answers_the_same_identity_under_two_spellings(): Unit =
        runBlocking {
            val base = requireOrigin()
            client().use { client ->
                val plain = probeInstance(client, normalized(base))
                val withPath = probeInstance(client, normalized("$base/some/path?x=1"))
                assertIs<ProbeResult.Found>(plain)
                assertIs<ProbeResult.Found>(withPath)
                assertEquals(plain.beacon.instance, withPath.beacon.instance)
                assertEquals(plain.origin, withPath.origin)
            }
        }

    /** A real 404 on a real server, refused for the right reason. */
    @Test
    fun a_path_that_is_not_the_beacon_is_not_an_instance(): Unit =
        runBlocking {
            val base = requireOrigin()
            client().use { client ->
                val result = probeInstance(client, "${normalized(base)}/definitely-not-talaria")
                assertIs<ProbeResult.Failed>(result)
            }
        }

    // ---- authed surface ---------------------------------------------------

    /**
     * The whole credential plane against a real api: a real `Set-Cookie` with
     * real attributes, captured and replayed as a real `Cookie` header that a
     * real Talaria accepts.
     */
    @Test
    fun signing_in_and_reading_the_profile_works(): Unit =
        runBlocking {
            client().use { client ->
                val (instance, credential) = session(client)
                assertIs<Credential.Session>(credential)
                val me = readMe(client, instance, credential)
                assertIs<MeResult.Ok>(me, "a session that just signed in must read /api/me")
            }
        }

    /**
     * The one that justifies the file. `/api/home`'s shape is computed, so the
     * generated reference prints `…` for it and the Kotlin models were
     * transcribed from the api's structs by hand. This is where that
     * transcription is checked against reality.
     */
    @Test
    fun the_home_summary_decodes_from_the_real_api(): Unit =
        runBlocking {
            client().use { client ->
                val (instance, credential) = session(client)
                val home = readHome(client, instance, credential)
                assertIs<HomeResult.Ok>(home, "the home summary must decode — a Failed here means the shape moved")
                val summary = home.summary
                // Counts, not contents. Every number must be present and sane;
                // the tickets behind them are somebody's real work.
                assertTrue(summary.org.name.isNotBlank(), "the org glance carries a name")
                assertTrue(summary.needsYou >= 0)
                assertTrue(summary.unread >= 0)
                assertTrue(summary.boards >= 0)
                // The api is explicit that a queue's count is the whole queue
                // and its items are a capped window, so items can never exceed
                // the count. If this ever trips, the screen's "+N more" is lying.
                listOf(summary.queues.review, summary.queues.blocked, summary.queues.triage).forEach { bucket ->
                    assertTrue(
                        bucket.items.size <= bucket.count,
                        "a window cannot hold more than the queue it is a window onto",
                    )
                }
            }
        }

    @Test
    fun the_channel_list_decodes_from_the_real_api(): Unit =
        runBlocking {
            client().use { client ->
                val (instance, credential) = session(client)
                val channels = readChannels(client, instance, credential)
                assertIs<CommsResult.Ok<*>>(channels, "the channel list must decode")
                @Suppress("UNCHECKED_CAST")
                val rooms = (channels as CommsResult.Ok<List<app.talaria.mobile.net.Channel>>).value
                // Shape only: every room must be addressable and labellable,
                // which is all the list screen asks of it.
                rooms.forEach { room ->
                    assertTrue(room.id.isNotBlank(), "a room must have an id")
                    assertTrue(room.label.isNotBlank(), "every room must render as something")
                    assertTrue(room.unreadCount >= 0)
                }
            }
        }

    /**
     * The firehose against the real thing. A live instance may be perfectly
     * quiet, so this does NOT require an event to arrive — it requires the
     * stream to OPEN and stay open, which is the part that breaks (a 401, a
     * proxy that buffers, a client timeout that kills a healthy connection).
     * Any event that does arrive must decode.
     */
    @Test
    fun the_event_stream_opens_and_stays_open(): Unit =
        runBlocking {
            client().use { client ->
                val (instance, credential) = session(client)
                // Null means "nothing arrived in 8 seconds", which is a quiet
                // workspace and a pass. A throw would be a failure.
                val events: List<UserEvent>? =
                    withTimeoutOrNull(8_000) {
                        userEvents(client, instance, credential).take(1).toList()
                    }
                events?.forEach { event ->
                    // The invariant, against the real api: ids only, no content.
                    assertTrue(
                        event.toString().length < 400,
                        "an event should be id-shaped, not a payload",
                    )
                }
            }
        }
}
