package app.talaria.mobile

import androidx.compose.runtime.Composable
import androidx.compose.runtime.remember
import app.talaria.mobile.account.AccountStore
import app.talaria.mobile.account.InMemoryAccountStore
import app.talaria.mobile.mercury.MercuryTheme
import app.talaria.mobile.net.Ports
import app.talaria.mobile.net.httpPorts
import app.talaria.mobile.net.talariaHttpClient
import app.talaria.mobile.screens.LauncherScreen
import app.talaria.mobile.screens.SignInScreen
import app.talaria.mobile.screens.SignedScreen

/**
 * The whole app, routed.
 *
 * Both parameters are injectable because that is what makes the headless UI
 * tests possible: a test hands in fake [Ports] and a seeded store, and then
 * drives the real screens (see `UiHarness.kt`). The phone's entry
 * points — `MainActivity` on Android, `MainViewController` on iOS — call this
 * with the defaults.
 */
@Composable
fun App(
    ports: Ports = remember { httpPorts(talariaHttpClient()) },
    store: AccountStore = remember { InMemoryAccountStore() },
) {
    val state = remember { AppState(ports, store) }
    MercuryTheme {
        when (val route = state.route) {
            is Route.Launcher -> LauncherScreen(state)
            is Route.SignIn -> SignInScreen(state, route.instance)
            is Route.Signed -> SignedScreen(state, route.account)
        }
    }
}
