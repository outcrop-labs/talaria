package app.talaria.mobile.net

import app.talaria.mobile.account.Instance
import io.ktor.client.statement.HttpResponse
import io.ktor.http.HttpStatusCode

/**
 * One place that turns a response into a result.
 *
 * Every authed read and write goes through here so that an expired session is
 * reported the same way everywhere and none of them can swallow it. That
 * matters more than it looks: the session has a 7-day absolute TTL with no
 * sliding refresh, so expiry is not an edge case — it happens to every account
 * on a schedule, and a screen that reported it as "something went wrong" would
 * leave someone tapping a dead app instead of signing in again.
 */
sealed interface CommsResult<out T> {
    data class Ok<T>(val value: T) : CommsResult<T>

    /** The credential is no longer good. The account should be signed out. */
    data object Expired : CommsResult<Nothing>

    data class Failed(val reason: String) : CommsResult<Nothing>
}

private fun HttpStatusCode.isSuccess() = value in 200..299

internal suspend fun <T> guardedCall(
    instance: Instance,
    call: suspend () -> HttpResponse,
    decode: suspend (HttpResponse) -> T,
): CommsResult<T> {
    val response =
        try {
            call()
        } catch (e: Throwable) {
            // Unreachable is NOT unauthorized. A dropped tunnel or a sleeping
            // radio must never cost somebody their session.
            return CommsResult.Failed("could not reach ${instance.label}: ${e.message ?: "no answer"}")
        }
    if (response.status == HttpStatusCode.Unauthorized || response.status == HttpStatusCode.Forbidden) {
        return CommsResult.Expired
    }
    if (!response.status.isSuccess()) {
        return CommsResult.Failed("${instance.label} answered ${response.status.value}")
    }
    return try {
        CommsResult.Ok(decode(response))
    } catch (_: Throwable) {
        CommsResult.Failed("${instance.label} answered something unreadable")
    }
}
