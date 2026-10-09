package app.talaria.mobile.account

import app.talaria.mobile.instance.Beacon
import kotlinx.serialization.Serializable

/**
 * The registry: which instances this phone knows, and which accounts are signed
 * in on each.
 *
 * Keyed the way the desktop registry is keyed — on the **beacon uuid**, not the
 * URL — so one instance reached by two names stays one instance. An account is
 * `(instance, user)`, which is what lets the same instance hold two signed-in
 * people, the case desktop structurally cannot serve.
 */
@Serializable
data class Instance(
    /** The beacon uuid. The identity, and the dedupe key. */
    val id: String,
    /** Normalized origin — scheme, host, non-default port. Nothing else. */
    val origin: String,
    /** `companyName` from the beacon, or host(:port). */
    val label: String,
) {
    companion object {
        fun of(
            origin: String,
            beacon: Beacon,
            label: String,
        ) = Instance(id = beacon.instance, origin = origin, label = label)
    }
}

@Serializable
data class Account(
    val instanceId: String,
    /** The email or username the person signed in with — the only stable handle
     *  the login response gives us before `/api/me` is read. */
    val username: String,
    /** Display name once `/api/me` has been read; the username until then. */
    val displayName: String? = null,
) {
    val key: String get() = "$instanceId/$username"
    val shown: String get() = displayName?.takeIf { it.isNotBlank() } ?: username
}

/**
 * Where instances, accounts and their credentials live.
 *
 * Credentials are deliberately a SEPARATE read from the records: the records
 * are ordinary data a list can render, and a credential is a secret that should
 * be fetched only at the moment a request needs it.
 *
 * The persistent implementations (Keychain on iOS, the Keystore on Android) land
 * with gap 1 rather than now, and the reason is sequencing rather than laziness:
 * gap 1 changes *what is stored* from a 7-day session id to a long-lived device
 * token, and secure storage written against the short-lived thing would be
 * rewritten the moment the long-lived one arrives. The in-memory implementation
 * below is what the tests use and what the app uses until then — so a restart
 * signs you out, which is honest about the state of the credential plane.
 */
interface AccountStore {
    fun instances(): List<Instance>

    fun accounts(): List<Account>

    fun credential(account: Account): Credential?

    fun addInstance(instance: Instance)

    fun removeInstance(instanceId: String)

    fun signIn(
        account: Account,
        credential: Credential,
    )

    fun signOut(account: Account)

    fun update(account: Account)
}

class InMemoryAccountStore(
    instances: List<Instance> = emptyList(),
    signedIn: Map<Account, Credential> = emptyMap(),
) : AccountStore {
    private val byInstanceId = instances.associateBy { it.id }.toMutableMap()
    private val creds = signedIn.mapKeys { it.key.key }.toMutableMap()
    private val byAccountKey = signedIn.keys.associateBy { it.key }.toMutableMap()

    override fun instances() = byInstanceId.values.toList()

    override fun accounts() = byAccountKey.values.toList()

    override fun credential(account: Account) = creds[account.key]

    override fun addInstance(instance: Instance) {
        byInstanceId[instance.id] = instance
    }

    override fun removeInstance(instanceId: String) {
        byInstanceId.remove(instanceId)
        // Removing an instance takes its accounts and their credentials with
        // it. A credential for an instance the app no longer knows is a secret
        // nothing can ever use and nothing can ever revoke.
        byAccountKey.values.filter { it.instanceId == instanceId }.forEach { signOut(it) }
    }

    override fun signIn(
        account: Account,
        credential: Credential,
    ) {
        byAccountKey[account.key] = account
        creds[account.key] = credential
    }

    override fun signOut(account: Account) {
        byAccountKey.remove(account.key)
        creds.remove(account.key)
    }

    override fun update(account: Account) {
        if (byAccountKey.containsKey(account.key)) byAccountKey[account.key] = account
    }
}
