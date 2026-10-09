package app.talaria.mobile

import io.ktor.client.HttpClient
import io.ktor.client.engine.mock.MockEngine
import io.ktor.client.engine.mock.MockRequestHandleScope
import io.ktor.client.engine.mock.respond
import io.ktor.client.plugins.contentnegotiation.ContentNegotiation
import io.ktor.client.request.HttpRequestData
import io.ktor.client.request.HttpResponseData
import io.ktor.http.ContentType
import io.ktor.http.HttpStatusCode
import io.ktor.http.headersOf
import io.ktor.serialization.kotlinx.json.json
import kotlinx.serialization.json.Json

/**
 * The headless UI driver, and what makes it possible to test screens here at
 * all.
 *
 * `runComposeUiTest` composes the REAL tree — the same composables the phone
 * runs — against Skia's software renderer. No display, no emulator, no device,
 * no screenshots to eyeball: it finds nodes, clicks them, types into them and
 * asserts, inside the ordinary `:shared:jvmTest` gate that already runs in a
 * devbox in seconds. That is the whole reason the UI lives in `commonMain`
 * rather than behind a platform boundary.
 *
 * What these tests are FOR: behaviour. Does typing a bad URL show a reason and
 * leave the list alone? Does a 404 beacon refuse to become an instance? Those
 * are the questions a screenshot cannot answer and a logic test cannot reach.
 * Appearance is the spec's job (docs/design/mercury-spec.md), not a test's.
 */

/** A Talaria api double. Routes are matched by path, in registration order, so
 *  a test states only the endpoints it cares about and any other call fails
 *  loudly rather than silently returning an empty body. */
class FakeApi {
    private val routes = mutableListOf<Pair<(HttpRequestData) -> Boolean, MockRequestHandleScope.(HttpRequestData) -> HttpResponseData>>()

    /** Every request this double saw, in order — the assertion surface for
     *  "did it actually call the endpoint, with the right credential?" */
    val seen = mutableListOf<HttpRequestData>()

    fun on(
        path: String,
        handler: MockRequestHandleScope.(HttpRequestData) -> HttpResponseData,
    ): FakeApi {
        routes += Pair({ r: HttpRequestData -> r.url.encodedPath == path }, handler)
        return this
    }

    fun json(
        path: String,
        body: String,
        status: HttpStatusCode = HttpStatusCode.OK,
    ): FakeApi =
        on(path) {
            respond(body, status, headersOf("Content-Type" to listOf(ContentType.Application.Json.toString())))
        }

    fun status(
        path: String,
        status: HttpStatusCode,
    ): FakeApi = on(path) { respond("", status) }

    fun client(): HttpClient {
        val engine =
            MockEngine { request ->
                seen += request
                val route = routes.firstOrNull { it.first(request) }
                    ?: error("FakeApi has no route for ${request.method.value} ${request.url.encodedPath}")
                route.second(this, request)
            }
        return HttpClient(engine) {
            expectSuccess = false
            install(ContentNegotiation) { json(Json { ignoreUnknownKeys = true; explicitNulls = false }) }
        }
    }
}

/** A beacon body for an instance that exists. */
fun beaconJson(
    instance: String = "3f2504e0-4f89-11d3-9a0c-0305e82c3301",
    companyName: String? = "Outcrop Labs",
): String =
    buildString {
        append("""{"instance":"$instance"""")
        if (companyName != null) append(""","companyName":"$companyName"""")
        append("}")
    }
