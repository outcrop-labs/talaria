package app.talaria.mobile.net

import app.talaria.mobile.account.Credential
import app.talaria.mobile.account.Instance
import app.talaria.mobile.account.MeResult
import app.talaria.mobile.account.SignInResult
import app.talaria.mobile.account.readMe
import app.talaria.mobile.account.signIn
import app.talaria.mobile.instance.ProbeResult
import app.talaria.mobile.instance.probeInstance
import io.ktor.client.HttpClient

/**
 * Everything the screens can ask the outside world for, as suspend functions.
 *
 * WHY A PORT BOUNDARY AND NOT AN HttpClient. Two reasons, and the second is the
 * one that forced it.
 *
 * 1. The UI layer imports no Ktor at all. A screen states what it needs
 *    ("sign this person in") rather than how it is fetched, so swapping the
 *    session cookie for a device credential, or adding a cache, happens here
 *    and not in a composable.
 *
 * 2. It makes the headless UI tests DETERMINISTIC. Driving a real client from a
 *    UI test means `waitUntil` is racing a coroutine on another dispatcher, and
 *    a virtual test clock can exhaust its timeout before that coroutine is
 *    scheduled — which showed up exactly as it always does, as tests that pass
 *    and fail on alternate runs for no visible reason. With a port, a UI test
 *    hands in a function that answers immediately and asserts UI behaviour
 *    only.
 *
 * What this does NOT do is let the wire go untested. [httpPorts] is the real
 * implementation, and `LoopbackApiTest` drives it against a JDK HTTP server
 * over a real socket — url building, header writing, `Set-Cookie` parsing and
 * JSON decoding included. The split is deliberate: UI behaviour is asserted in
 * the UI, wire behaviour is asserted on the wire, and neither test is flaky
 * because neither is waiting on the other's scheduler.
 */
data class Ports(
    val probe: suspend (origin: String) -> ProbeResult,
    val signIn: suspend (instance: Instance, username: String, password: String) -> SignInResult,
    val me: suspend (instance: Instance, credential: Credential) -> MeResult,
    val home: suspend (instance: Instance, credential: Credential) -> HomeResult,
)

/** The real ports, over HTTP. */
fun httpPorts(client: HttpClient): Ports =
    Ports(
        probe = { origin -> probeInstance(client, origin) },
        signIn = { instance, username, password -> signIn(client, instance, username, password) },
        me = { instance, credential -> readMe(client, instance, credential) },
        home = { instance, credential -> readHome(client, instance, credential) },
    )
