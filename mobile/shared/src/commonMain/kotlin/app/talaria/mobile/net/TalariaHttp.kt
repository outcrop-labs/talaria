package app.talaria.mobile.net

import io.ktor.client.HttpClient
import io.ktor.client.plugins.HttpTimeout
import io.ktor.client.plugins.contentnegotiation.ContentNegotiation
import io.ktor.serialization.kotlinx.json.json
import kotlinx.serialization.json.Json

/**
 * One HTTP client, no platform shims.
 *
 * There is no expect/actual engine here on purpose: Ktor picks the engine off
 * the classpath per target, so `ktor-client-okhttp` in androidMain and
 * `ktor-client-darwin` in iosMain is the whole platform story. Both stream
 * responses natively, which is the reason this app is Kotlin at all — the
 * decision queue, messaging and agent chat all ride SSE (docs/MOBILE.md).
 *
 * NOTE ON AUTH: nothing here attaches a credential. Requests carry their own
 * account's credential explicitly, because this client serves MANY accounts
 * across many instances at once and an ambient, client-wide token is precisely
 * the leak the desktop shell avoids by giving each instance its own cookie jar.
 * A phone has one process, so the isolation has to be per-request.
 */
fun talariaHttpClient(): HttpClient =
    HttpClient {
        expectSuccess = false // statuses are read, not thrown: a 404 beacon is an answer.
        install(ContentNegotiation) {
            json(
                Json {
                    // The api adds fields without asking the client's permission,
                    // and an older build must not start failing when it does.
                    ignoreUnknownKeys = true
                    explicitNulls = false
                },
            )
        }
        install(HttpTimeout) {
            // Deliberately short: this is a phone on somebody's commute, and a
            // request that is going to fail should fail while the screen is
            // still in their hand. Streaming requests override this per-call.
            requestTimeoutMillis = 20_000
            connectTimeoutMillis = 10_000
        }
    }
