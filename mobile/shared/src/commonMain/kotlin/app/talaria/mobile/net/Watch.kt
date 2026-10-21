package app.talaria.mobile.net

import app.talaria.mobile.account.Credential
import app.talaria.mobile.account.Instance
import io.ktor.client.HttpClient
import io.ktor.client.call.body
import io.ktor.client.request.get
import kotlinx.serialization.SerialName
import kotlinx.serialization.Serializable

/**
 * What an agent is doing right now, and what the workforce is costing.
 *
 * Two reads that answer the same shape of question — *is this worth it?* — and
 * both are information rather than action, which is why they sit together and
 * why neither gets a button.
 *
 * `GET /api/tasks/{id}/work-session` is the live session on a ticket: the run
 * row plus the tail of the agent's last reply, read from the checkpoint the
 * session persists before acting. The reply tail is the only place in this app
 * where an agent's *words* arrive without a separate fetch — and that is fine,
 * because it is the ticket's own run and the route is ACL'd on the ticket.
 *
 * `GET /api/cost` is the token ledger, and it needs the `view:/observability`
 * grant. A 403 here is therefore NORMAL for most members and must read as "not
 * for you" rather than as a failure: see [CostResult.Forbidden].
 */
@Serializable
data class WorkSession(
    @SerialName("runId") val runId: String = "",
    val state: String? = null,
    val phase: String? = null,
    @SerialName("agentModel") val agentModel: String? = null,
    val turn: Int? = null,
    /** The tail of the agent's last reply, from its own checkpoint. */
    @SerialName("lastTail") val lastTail: String? = null,
)

@Serializable
data class WorkSessionRead(
    /** Null when nothing is running on this ticket — a real answer, not an
     *  absence. */
    val session: WorkSession? = null,
    /** The api's own hint about how long to wait before asking again. */
    val wait: Int? = null,
)

@Serializable
data class CostWindow(
    val prompt: Int = 0,
    val completion: Int = 0,
    val cache: Int = 0,
    val generations: Int = 0,
    /** Arrives as a JSON number; `js_numberify` means a whole dollar prints as
     *  `3` rather than `3.0`, which decodes to Double either way. */
    val cost: Double = 0.0,
) {
    val tokens: Int get() = prompt + completion
}

@Serializable
data class CostPerAgent(
    @SerialName("agentModel") val agentModel: String? = null,
    val prompt: Int = 0,
    val completion: Int = 0,
    val generations: Int = 0,
    @SerialName("lastUsed") val lastUsed: String? = null,
    val cost: Double = 0.0,
) {
    val tokens: Int get() = prompt + completion
    val shown: String get() = agentModel?.takeIf { it.isNotBlank() } ?: "unattributed"
}

@Serializable
data class CostTotals(
    val today: CostWindow = CostWindow(),
    val week: CostWindow = CostWindow(),
    val month: CostWindow = CostWindow(),
)

@Serializable
data class CostOverview(
    val totals: CostTotals = CostTotals(),
    @SerialName("perAgent") val perAgent: List<CostPerAgent> = emptyList(),
)

const val COST_PATH = "/api/cost"

fun workSessionPath(taskId: String) = "/api/tasks/$taskId/work-session"

/**
 * The cost read's own result type, because it has a third outcome the others
 * do not: forbidden.
 *
 * Folding a 403 into `Failed` would put "Outcrop Labs answered 403" in front of
 * every member who does not hold the Observability view — which is most of
 * them, and which is the permission model working exactly as intended rather
 * than anything going wrong.
 */
sealed interface CostResult {
    data class Ok(val overview: CostOverview) : CostResult

    /** No `view:/observability`. Normal, and not an error. */
    data object Forbidden : CostResult

    data object Expired : CostResult

    data class Failed(val reason: String) : CostResult
}

suspend fun readCost(
    client: HttpClient,
    instance: Instance,
    credential: Credential,
): CostResult {
    // Deliberately NOT guardedCall: that helper maps 401 and 403 together onto
    // Expired, which is right everywhere else and wrong here. A 403 on this
    // route means the person lacks a view, not that their session died —
    // signing them out over it would be a bug with a very confusing shape.
    val response =
        try {
            client.get("${instance.origin}$COST_PATH") { credential.applyTo(this) }
        } catch (e: Throwable) {
            return CostResult.Failed("could not reach ${instance.label}: ${e.message ?: "no answer"}")
        }
    return when (response.status.value) {
        in 200..299 ->
            try {
                CostResult.Ok(response.body())
            } catch (_: Throwable) {
                CostResult.Failed("${instance.label} answered an unreadable ledger")
            }
        401 -> CostResult.Expired
        403 -> CostResult.Forbidden
        else -> CostResult.Failed("${instance.label} answered ${response.status.value}")
    }
}

suspend fun readWorkSession(
    client: HttpClient,
    instance: Instance,
    credential: Credential,
    taskId: String,
): CommsResult<WorkSessionRead> =
    guardedCall(
        instance,
        call = { client.get("${instance.origin}${workSessionPath(taskId)}") { credential.applyTo(this) } },
        decode = { it.body<WorkSessionRead>() },
    )
