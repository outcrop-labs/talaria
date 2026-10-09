package app.talaria.mobile

import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateListOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.compose.runtime.snapshots.SnapshotStateList
import app.talaria.mobile.account.Account
import app.talaria.mobile.account.AccountStore
import app.talaria.mobile.account.Credential
import app.talaria.mobile.account.InMemoryAccountStore
import app.talaria.mobile.account.Instance
import app.talaria.mobile.net.Ports

/**
 * Where the app is and what it knows.
 *
 * TWO LAYERS ON PURPOSE. [AccountStore] is persistence — a plain interface that
 * knows nothing about Compose. This class is the OBSERVABLE mirror of it, so a
 * screen recomposes when an instance is added. Writes go through here to both.
 * Without the split, either the store would have to depend on Compose (making
 * it untestable from a plain unit test, and unimplementable against Keychain)
 * or the UI would never notice a change.
 *
 * Navigation is a `Route` value rather than a navigation library. There are a
 * handful of destinations and the whole point of the app is to be shallow — a
 * library would add a dependency, a backstack abstraction and a testing shim to
 * express `when (route)`.
 */
/** The signed-in surfaces. Two for now; the doc's phases add the rest. */
enum class SignedTab { Home, Comms }

sealed interface Route {
    /** The launcher: add an instance, or pick an account to open. */
    data object Launcher : Route

    data class SignIn(val instance: Instance) : Route

    data class Signed(val account: Account) : Route
}

class AppState(
    val ports: Ports,
    val store: AccountStore = InMemoryAccountStore(),
) {
    var route: Route by mutableStateOf<Route>(Route.Launcher)
        private set

    /** The observable mirrors. Seeded from the store so a caller can hand in a
     *  pre-populated one (which is how tests start mid-flow). */
    val instances: SnapshotStateList<Instance> = mutableStateListOf<Instance>().also { it.addAll(store.instances()) }
    val accounts: SnapshotStateList<Account> = mutableStateListOf<Account>().also { it.addAll(store.accounts()) }

    init {
        // Land on what is waiting, not on a chooser. One signed-in account is
        // the overwhelmingly common case for a controller app, and the desktop
        // shell makes the same choice when it reopens the instance you used
        // last. Two or more accounts is genuinely a question, so it is asked.
        val only = accounts.singleOrNull()
        if (only != null && store.credential(only) != null) route = Route.Signed(only)
    }

    /** Which signed-in surface is showing. */
    var tab: SignedTab by mutableStateOf(SignedTab.Home)
        private set

    /** The channel being read, if any. Null is the channel list. */
    var openChannelId: String? by mutableStateOf(null)
        private set

    fun go(to: Route) {
        route = to
        // Leaving an account closes whatever was open inside it: arriving back
        // later on a channel you did not choose is disorienting.
        openChannelId = null
    }

    fun show(tab: SignedTab) {
        this.tab = tab
        if (tab != SignedTab.Comms) openChannelId = null
    }

    fun openChannel(id: String?) {
        openChannelId = id
    }

    fun instanceOf(account: Account): Instance? = instances.firstOrNull { it.id == account.instanceId }

    fun accountsOn(instanceId: String): List<Account> = accounts.filter { it.instanceId == instanceId }

    fun credential(account: Account): Credential? = store.credential(account)

    /** Returns false when the instance was already known — the beacon uuid is
     *  the dedupe key, so one instance reached by two names is one row. */
    fun addInstance(instance: Instance): Boolean {
        if (instances.any { it.id == instance.id }) return false
        store.addInstance(instance)
        instances.add(instance)
        return true
    }

    fun removeInstance(instance: Instance) {
        store.removeInstance(instance.id)
        instances.removeAll { it.id == instance.id }
        accounts.removeAll { it.instanceId == instance.id }
    }

    fun signIn(
        account: Account,
        credential: Credential,
    ) {
        store.signIn(account, credential)
        accounts.removeAll { it.key == account.key }
        accounts.add(account)
    }

    fun signOut(account: Account) {
        store.signOut(account)
        accounts.removeAll { it.key == account.key }
        if ((route as? Route.Signed)?.account?.key == account.key) route = Route.Launcher
    }

    /** Why the launcher is showing, when it is showing for a reason — an
     *  expired session, say. Cleared as soon as it has been read once, because
     *  a notice that outlives its cause is noise. */
    var notice: String? by mutableStateOf(null)
        private set

    fun notice(message: String?) {
        notice = message
    }

    fun rename(
        account: Account,
        displayName: String?,
    ) {
        val next = account.copy(displayName = displayName)
        store.update(next)
        val at = accounts.indexOfFirst { it.key == account.key }
        if (at >= 0) accounts[at] = next
        if ((route as? Route.Signed)?.account?.key == account.key) route = Route.Signed(next)
    }
}
