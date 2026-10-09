package app.talaria.mobile.net

import app.talaria.mobile.account.Credential
import app.talaria.mobile.account.Instance
import io.ktor.client.HttpClient
import io.ktor.client.call.body
import io.ktor.client.request.get
import io.ktor.http.HttpStatusCode
import kotlinx.serialization.SerialName
import kotlinx.serialization.Serializable

/**
 * `GET /api/home` — "what is waiting on you", and the app's landing surface.
 *
 * The shapes below are transcribed from the api's own structs
 * (`api/crates/talaria-home/src/lib.rs`: `HomeSummary`, `HomeQueues`,
 * `QueueBucket`, `WorkItem`, `OrgGlance`) rather than guessed from the
 * generated reference, which prints `…` for this route because the shape is
 * computed. Note which structs carry `rename_all = "camelCase"` and which do
 * not — `WorkItem` and `OrgGlance` do (`boardId`, `ticketRef`, `updatedAt`,
 * `costToday`), `HomeSummary` and `CostToday` do not need to.
 *
 * THIS IS THE DECISION QUEUE. `queues.review` is the review gate, `blocked` is
 * work an agent cannot finish without a person, and `triage` is what has not
 * been looked at. Those three are the reason the app exists (docs/MOBILE.md).
 *
 * MODELLED: only what a screen renders. `org.activity` is on the wire and is
 * deliberately absent here — `ignoreUnknownKeys` drops it. A field modelled but
 * unrendered is a field nobody notices breaking.
 */
@Serializable
data class WorkItem(
    val id: String,
    @SerialName("boardId") val boardId: String,
    val board: String,
    @SerialName("ticketRef") val ticketRef: String? = null,
    val title: String,
    val status: String,
    @SerialName("updatedAt") val updatedAt: String? = null,
    val queue: String? = null,
) {
    /** `TASK-12 · Fix the login loop`, or the bare title when the board has no
     *  numbering — the same line the digest writes (`WorkItem::label`). */
    val label: String get() = ticketRef?.takeIf { it.isNotEmpty() }?.let { "$it · $title" } ?: title
}

@Serializable
data class QueueBucket(
    /** Everything in the queue — deliberately NOT `items.size`, which is the
     *  same number only until a queue outgrows the window. The api says so in
     *  as many words. */
    val count: Int = 0,
    val items: List<WorkItem> = emptyList(),
)

@Serializable
data class HomeQueues(
    val triage: QueueBucket = QueueBucket(),
    val review: QueueBucket = QueueBucket(),
    val blocked: QueueBucket = QueueBucket(),
)

@Serializable
data class CostToday(
    val tokens: Int = 0,
    val usd: Double = 0.0,
)

@Serializable
data class OrgGlance(
    val name: String = "",
    /** Admin-only; null for members. */
    val alerts: Int? = null,
    @SerialName("costToday") val costToday: CostToday? = null,
)

@Serializable
data class HomeSummary(
    val org: OrgGlance = OrgGlance(),
    val queues: HomeQueues = HomeQueues(),
    val unread: Int = 0,
    val boards: Int = 0,
) {
    /** The one number the landing surface leads with. Unread is excluded on
     *  purpose: a message is not a decision, and conflating the two is how a
     *  "needs you" badge stops meaning anything. */
    val needsYou: Int get() = queues.review.count + queues.blocked.count + queues.triage.count
}

const val HOME_PATH = "/api/home"

sealed interface HomeResult {
    data class Ok(val summary: HomeSummary) : HomeResult

    data object Expired : HomeResult

    data class Failed(val reason: String) : HomeResult
}

suspend fun readHome(
    client: HttpClient,
    instance: Instance,
    credential: Credential,
): HomeResult {
    val response =
        try {
            client.get("${instance.origin}$HOME_PATH") { credential.applyTo(this) }
        } catch (e: Throwable) {
            return HomeResult.Failed("could not reach ${instance.label}: ${e.message ?: "no answer"}")
        }
    if (response.status == HttpStatusCode.Unauthorized || response.status == HttpStatusCode.Forbidden) {
        return HomeResult.Expired
    }
    if (response.status != HttpStatusCode.OK) {
        return HomeResult.Failed("${instance.label} answered ${response.status.value}")
    }
    return try {
        HomeResult.Ok(response.body())
    } catch (_: Throwable) {
        HomeResult.Failed("${instance.label} did not answer a home summary")
    }
}
