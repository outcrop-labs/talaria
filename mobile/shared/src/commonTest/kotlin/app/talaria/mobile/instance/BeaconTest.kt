package app.talaria.mobile.instance

import io.ktor.client.HttpClient
import io.ktor.client.engine.mock.MockEngine
import io.ktor.client.engine.mock.respond
import io.ktor.client.engine.mock.respondError
import io.ktor.client.plugins.contentnegotiation.ContentNegotiation
import io.ktor.http.ContentType
import io.ktor.http.HttpStatusCode
import io.ktor.http.headersOf
import io.ktor.serialization.kotlinx.json.json
import kotlinx.coroutines.test.runTest
import kotlinx.serialization.json.Json
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertIs
import kotlin.test.assertTrue

/**
 * What makes a URL a Talaria instance is a VALID BEACON UUID — not a 200, and
 * not something answering on 443. Each case below is a way the check can be
 * fooled if the uuid is taken on trust.
 */
class BeaconTest {
    private val uuid = "3f2504e0-4f89-11d3-9a0c-0305e82c3301"

    private fun client(handler: MockEngine) =
        HttpClient(handler) {
            expectSuccess = false
            install(ContentNegotiation) { json(Json { ignoreUnknownKeys = true }) }
        }

    private fun json(body: String) =
        MockEngine {
            respond(body, HttpStatusCode.OK, headersOf("Content-Type" to listOf(ContentType.Application.Json.toString())))
        }

    @Test
    fun a_beacon_with_a_uuid_is_an_instance() =
        runTest {
            val result =
                probeInstance(
                    client(json("""{"instance":"$uuid","companyName":"Outcrop Labs"}""")),
                    "https://talaria.example",
                )
            assertIs<ProbeResult.Found>(result)
            assertEquals(uuid, result.beacon.instance)
            assertEquals("Outcrop Labs", result.label)
        }

    @Test
    fun the_path_asked_for_is_the_beacon_path() =
        runTest {
            var asked: String? = null
            val engine =
                MockEngine { request ->
                    asked = request.url.encodedPath
                    respond(
                        """{"instance":"$uuid"}""",
                        HttpStatusCode.OK,
                        headersOf("Content-Type" to listOf(ContentType.Application.Json.toString())),
                    )
                }
            probeInstance(client(engine), "https://talaria.example")
            assertEquals(BEACON_PATH, asked)
        }

    /** No company name: the label falls back to host(:port), as the desktop
     *  launcher's list does. */
    @Test
    fun a_nameless_instance_is_labelled_by_host() =
        runTest {
            val result = probeInstance(client(json("""{"instance":"$uuid"}""")), "http://127.0.0.1:5302")
            assertIs<ProbeResult.Found>(result)
            assertEquals("127.0.0.1:5302", result.label)
        }

    @Test
    fun a_blank_company_name_falls_back_too() =
        runTest {
            val result = probeInstance(client(json("""{"instance":"$uuid","companyName":"  "}""")), "https://x.example")
            assertIs<ProbeResult.Found>(result)
            assertEquals("x.example", result.label)
        }

    @Test
    fun something_that_is_not_a_uuid_is_not_an_instance() =
        runTest {
            val result = probeInstance(client(json("""{"instance":"definitely-not-a-uuid"}""")), "https://x.example")
            assertIs<ProbeResult.Failed>(result)
            assertTrue(result.reason.contains("not like a Talaria instance"))
        }

    @Test
    fun a_non_success_status_is_not_an_instance() =
        runTest {
            val result =
                probeInstance(
                    client(MockEngine { respondError(HttpStatusCode.NotFound) }),
                    "https://not-talaria.example",
                )
            assertIs<ProbeResult.Failed>(result)
            assertTrue(result.reason.contains("404"), "the reason should name the status: ${result.reason}")
        }

    @Test
    fun a_body_that_is_not_the_beacon_is_not_an_instance() =
        runTest {
            val result = probeInstance(client(json("""{"hello":"world"}""")), "https://x.example")
            assertIs<ProbeResult.Failed>(result)
        }

    /** HTML with a 200 is the commonest false positive: any web server at all. */
    @Test
    fun a_web_server_answering_html_is_not_an_instance() =
        runTest {
            val engine =
                MockEngine {
                    respond(
                        "<!doctype html><title>nginx</title>",
                        HttpStatusCode.OK,
                        headersOf("Content-Type" to listOf(ContentType.Text.Html.toString())),
                    )
                }
            assertIs<ProbeResult.Failed>(probeInstance(client(engine), "https://nginx.example"))
        }

    @Test
    fun an_unreachable_origin_says_so() =
        runTest {
            val engine = MockEngine { throw RuntimeException("dns go boom") }
            val result = probeInstance(client(engine), "https://nope.example")
            assertIs<ProbeResult.Failed>(result)
            assertTrue(result.reason.contains("could not reach"))
        }
}
