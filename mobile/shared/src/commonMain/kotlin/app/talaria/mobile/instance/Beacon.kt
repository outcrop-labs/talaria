package app.talaria.mobile.instance

import io.ktor.client.HttpClient
import io.ktor.client.call.body
import io.ktor.client.request.get
import io.ktor.client.statement.HttpResponse
import io.ktor.http.isSuccess
import kotlinx.serialization.SerialName
import kotlinx.serialization.Serializable

/**
 * Instance discovery. A valid beacon uuid is what makes a URL "a Talaria
 * instance" — not just something that answers on 443. Served by
 * `api/crates/talaria-routes-workbench/src/system/well_known_talaria_instance.rs`;
 * the desktop shell asks the identical question in
 * `desktop/src-tauri/src/beacon.rs`.
 */
const val BEACON_PATH = "/api/well-known/talaria-instance"

@Serializable
data class Beacon(
    val instance: String,
    @SerialName("companyName") val companyName: String? = null,
)

/** The uuid shape, checked rather than parsed: Kotlin's own `Uuid` is still
 *  behind an opt-in annotation, and a regex says exactly as much about a
 *  beacon's honesty as a parse would. */
private val UUID =
    Regex("^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$")

sealed interface ProbeResult {
    data class Found(val origin: String, val beacon: Beacon) : ProbeResult {
        /** `companyName` when the instance states one, host(:port) when it does
         *  not — the desktop launcher's rule, so one instance reads the same on
         *  both. */
        val label: String get() = beacon.companyName?.takeIf { it.isNotBlank() } ?: hostLabel(origin)
    }

    data class Failed(val reason: String) : ProbeResult
}

/**
 * Ask an origin whether it is a Talaria instance.
 *
 * Every failure is a sentence someone can act on, which is why this returns a
 * result rather than throwing: "it answered 404" and "the TLS name is wrong"
 * send a person to different places, and an exception type would flatten both
 * into "could not add instance".
 */
suspend fun probeInstance(
    client: HttpClient,
    origin: String,
): ProbeResult {
    val response: HttpResponse =
        try {
            client.get("$origin$BEACON_PATH")
        } catch (e: Throwable) {
            return ProbeResult.Failed("could not reach $origin: ${e.message ?: "no answer"}")
        }

    if (!response.status.isSuccess()) {
        return ProbeResult.Failed(
            "$origin answered ${response.status.value} at $BEACON_PATH — " +
                "it does not look like a Talaria instance",
        )
    }

    val beacon =
        try {
            response.body<Beacon>()
        } catch (_: Throwable) {
            return ProbeResult.Failed("$origin did not answer instance info")
        }

    if (!UUID.matches(beacon.instance)) {
        return ProbeResult.Failed("$origin answered, but not like a Talaria instance")
    }
    return ProbeResult.Found(origin, beacon)
}
