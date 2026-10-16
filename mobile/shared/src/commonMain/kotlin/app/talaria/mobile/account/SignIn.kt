package app.talaria.mobile.account

import io.ktor.client.HttpClient
import io.ktor.client.call.body
import io.ktor.client.request.get
import io.ktor.client.request.post
import io.ktor.client.request.setBody
import io.ktor.client.statement.HttpResponse
import io.ktor.client.statement.bodyAsText
import io.ktor.http.ContentType
import io.ktor.http.HttpStatusCode
import io.ktor.http.contentType
import io.ktor.http.setCookie
import kotlinx.serialization.SerialName
import kotlinx.serialization.Serializable

/**
 * Password sign-in, and the one read that proves a credential still works.
 *
 * The api's own spelling is `{ username, password }` — not `email`, which is
 * what `/api/auth/claim` takes. Guessing from the wrong neighbour is the sort
 * of mistake that produces a 400 nobody can explain, so the field names here
 * come from `docs/api/account.md`.
 *
 * Google sign-in is deliberately absent. The OAuth callback sets a browser
 * cookie and 302s to a web path; there is no custom-scheme redirect to hand
 * back to an app, so in-app Google needs api work and arrives with gap 1.
 */
@Serializable
private data class PasswordBody(
    val username: String,
    val password: String,
)

@Serializable
data class Me(
    @SerialName("preferredModel") val preferredModel: String? = null,
    @SerialName("preferredEffort") val preferredEffort: String? = null,
    val timezone: String? = null,
    val title: String? = null,
    val name: String? = null,
)

sealed interface SignInResult {
    data class Ok(
        val account: Account,
        val credential: Credential,
    ) : SignInResult

    data class Refused(val reason: String) : SignInResult
}

const val PASSWORD_PATH = "/api/auth/password"
const val ME_PATH = "/api/me"

suspend fun signIn(
    client: HttpClient,
    instance: Instance,
    username: String,
    password: String,
): SignInResult {
    val response: HttpResponse =
        try {
            client.post("${instance.origin}$PASSWORD_PATH") {
                contentType(ContentType.Application.Json)
                setBody(PasswordBody(username = username.trim(), password = password))
            }
        } catch (e: Throwable) {
            return SignInResult.Refused("could not reach ${instance.label}: ${e.message ?: "no answer"}")
        }

    // Each status says something different to the person holding the phone, and
    // flattening them into "sign-in failed" would hide the one case they can
    // actually act on.
    when (response.status) {
        HttpStatusCode.OK -> Unit
        HttpStatusCode.Unauthorized -> return SignInResult.Refused("that username and password did not match")
        HttpStatusCode.TooManyRequests -> return SignInResult.Refused("too many attempts — wait a moment and try again")
        HttpStatusCode.BadRequest -> return SignInResult.Refused("the instance refused the request")
        else -> return SignInResult.Refused("${instance.label} answered ${response.status.value}")
    }

    // The session id arrives ONLY as a Set-Cookie, so a 200 with no such cookie
    // is a failure however healthy it looks — and `HttpCookies` is deliberately
    // not installed on this client, so nothing captured it behind our back.
    val sid =
        response.setCookie().firstOrNull { it.name == Credential.COOKIE_NAME }?.value
            ?: return SignInResult.Refused("the instance signed in without issuing a session")

    return SignInResult.Ok(
        account = Account(instanceId = instance.id, username = username.trim()),
        credential = Credential.Session(sid),
    )
}

/**
 * Read the signed-in person's profile. Doubles as the liveness check for a
 * stored credential: a 401 here is how the app learns a session expired, which
 * on a 7-day absolute TTL is a thing that happens on a schedule.
 */
sealed interface MeResult {
    data class Ok(val me: Me) : MeResult

    /** The credential is no longer good. The account should be signed out. */
    data object Expired : MeResult

    data class Failed(val reason: String) : MeResult
}

suspend fun readMe(
    client: HttpClient,
    instance: Instance,
    credential: Credential,
): MeResult {
    val response =
        try {
            client.get("${instance.origin}$ME_PATH") { credential.applyTo(this) }
        } catch (e: Throwable) {
            return MeResult.Failed("could not reach ${instance.label}: ${e.message ?: "no answer"}")
        }
    if (response.status == HttpStatusCode.Unauthorized || response.status == HttpStatusCode.Forbidden) {
        return MeResult.Expired
    }
    if (response.status != HttpStatusCode.OK) {
        return MeResult.Failed("${instance.label} answered ${response.status.value}")
    }
    return try {
        MeResult.Ok(response.body())
    } catch (_: Throwable) {
        MeResult.Failed("the instance did not answer a profile: ${response.bodyAsText().take(80)}")
    }
}
