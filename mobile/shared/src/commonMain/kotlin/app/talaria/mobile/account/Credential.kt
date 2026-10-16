package app.talaria.mobile.account

import io.ktor.client.request.HttpRequestBuilder
import io.ktor.client.request.header

/**
 * What proves who an account is, on every single request.
 *
 * THE RULE THIS TYPE EXISTS TO ENFORCE: there is no ambient credential. The
 * desktop shell gets account isolation for free, because each instance webview
 * owns its own cookie jar and the OS keeps them apart. A phone is one process
 * with one HTTP stack, so an app-wide cookie jar would be a hole — account A's
 * cookie riding a request to instance B. Here a credential is a VALUE attached
 * per call, which is why two accounts on the *same* instance work as naturally
 * as two instances do, and desktop cannot do that.
 *
 * Today's only implementation replays the session cookie, because that is what
 * the api issues. [`docs/MOBILE.md`](../../../../../../../docs/MOBILE.md) calls
 * the replacement gap 1: a `tdv_…` device credential, because the session is a
 * 7-day absolute TTL with no sliding refresh and a controller that signs you
 * out while you are away defeats itself. When that lands it becomes a second
 * implementation of this interface and nothing above it changes — which is the
 * whole point of naming the seam now.
 */
sealed interface Credential {
    /** Attach this credential to one outgoing request. */
    fun applyTo(request: HttpRequestBuilder)

    /** An opaque session id from `POST /api/auth/password`, replayed as the
     *  cookie the api sets. The cookie name is the api's own
     *  (`talaria-session/src/lib.rs`: `SESSION_COOKIE`). */
    data class Session(val sid: String) : Credential {
        override fun applyTo(request: HttpRequestBuilder) {
            // Set as a raw header rather than through a cookie store ON
            // PURPOSE: a store is shared state, and shared state is how one
            // account's credential reaches another account's request.
            request.header("Cookie", "$COOKIE_NAME=$sid")
        }
    }

    /** A per-device token (gap 1), not yet issued by any api. Kept here as a
     *  declaration of where it plugs in, not as a working credential. */
    data class Device(val token: String) : Credential {
        override fun applyTo(request: HttpRequestBuilder) {
            request.header("Authorization", "Bearer $token")
        }
    }

    companion object {
        const val COOKIE_NAME = "talaria_session"
    }
}
