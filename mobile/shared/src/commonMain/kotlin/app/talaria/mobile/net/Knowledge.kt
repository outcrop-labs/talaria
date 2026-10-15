package app.talaria.mobile.net

import app.talaria.mobile.account.Credential
import app.talaria.mobile.account.Instance
import io.ktor.client.HttpClient
import io.ktor.client.call.body
import io.ktor.client.request.get
import io.ktor.client.request.parameter
import kotlinx.serialization.SerialName
import kotlinx.serialization.Serializable

/**
 * Reading: knowledge docs, and a plan's living document.
 *
 * READING ONLY, and that is the scope line rather than a staging post.
 * Knowledge is a versioned markdown drive feeding visibility-scoped RAG
 * brains; editing one on a phone means a diff, a merge story and an undo
 * story, which is a desk. What a phone needs is the paragraph that explains
 * the ticket it is about to approve.
 *
 * Shapes from `talaria-kb` (`KbDoc`, `KbSearchHit`, both camelCase) and
 * `talaria-artifacts` (`Artifact`). A plan's document is an artifact, reached
 * through `GET /api/plans/{id}/doc`, and the plan itself is a conversation —
 * there is no plan list route, so `GET /api/conversations?kind=plan` is the
 * way in (`talaria-conversations`' `ConversationListRow`).
 */
@Serializable
data class KbSearchHit(
    val id: String,
    @SerialName("spaceId") val spaceId: String = "",
    @SerialName("spaceName") val spaceName: String = "",
    val title: String = "",
    val icon: String? = null,
    val visibility: String = "",
    /** Spaces are documents too — a hit of kind `space` opens the space's own
     *  overview rather than a doc inside it. */
    val kind: String = "",
    /**
     * PRE-ESCAPED HTML carrying `<b>` highlights, not plain text. The api owns
     * that escaping on purpose (`talaria-kb`'s `safe_snippet`); rendering this
     * as a raw string shows a reader literal tags. Use
     * `app.talaria.mobile.text.snippetToAnnotated`.
     */
    val snippet: String = "",
)

@Serializable
data class KbDoc(
    val id: String,
    @SerialName("spaceId") val spaceId: String = "",
    val title: String = "",
    val icon: String? = null,
    val body: String = "",
    val kind: String = "",
    val official: Boolean = false,
    val visibility: String = "",
    @SerialName("updatedAt") val updatedAt: String? = null,
)

@Serializable
data class Artifact(
    val id: String,
    val kind: String = "",
    val title: String = "",
    val body: String = "",
    val visibility: String = "",
    @SerialName("updatedAt") val updatedAt: String? = null,
)

@Serializable
data class PlanSummary(
    val id: String,
    @SerialName("agentModel") val agentModel: String = "",
    val title: String? = null,
    @SerialName("updatedAt") val updatedAt: String? = null,
    /** An assistant reply is streaming right now. */
    val working: Boolean = false,
    /** The latest assistant turn errored — the thread needs a re-run. */
    val failed: Boolean = false,
    @SerialName("ownerLabel") val ownerLabel: String? = null,
    @SerialName("unreadCount") val unreadCount: Int = 0,
) {
    val shown: String get() = title?.takeIf { it.isNotBlank() } ?: "Untitled plan"
}

@Serializable
private data class HitsEnvelope(val hits: List<KbSearchHit> = emptyList())

@Serializable
private data class DocEnvelope(val doc: KbDoc)

@Serializable
private data class ArtifactEnvelope(val artifact: Artifact)

@Serializable
private data class ConversationsEnvelope(
    val conversations: List<PlanSummary> = emptyList(),
)

const val KB_SEARCH_PATH = "/api/kb/search"
const val CONVERSATIONS_PATH = "/api/conversations"

fun kbDocPath(id: String) = "/api/kb/docs/$id"

fun planDocPath(id: String) = "/api/plans/$id/doc"

suspend fun searchKnowledge(
    client: HttpClient,
    instance: Instance,
    credential: Credential,
    query: String,
): CommsResult<List<KbSearchHit>> =
    guardedCall(
        instance,
        call = {
            client.get("${instance.origin}$KB_SEARCH_PATH") {
                credential.applyTo(this)
                parameter("q", query)
            }
        },
        decode = { it.body<HitsEnvelope>().hits },
    )

suspend fun readKbDoc(
    client: HttpClient,
    instance: Instance,
    credential: Credential,
    docId: String,
): CommsResult<KbDoc> =
    guardedCall(
        instance,
        call = { client.get("${instance.origin}${kbDocPath(docId)}") { credential.applyTo(this) } },
        decode = { it.body<DocEnvelope>().doc },
    )

/** Plans are conversations, filtered by kind. There is no plan list route. */
suspend fun listPlans(
    client: HttpClient,
    instance: Instance,
    credential: Credential,
): CommsResult<List<PlanSummary>> =
    guardedCall(
        instance,
        call = {
            client.get("${instance.origin}$CONVERSATIONS_PATH") {
                credential.applyTo(this)
                parameter("kind", "plan")
            }
        },
        decode = { it.body<ConversationsEnvelope>().conversations },
    )

/**
 * A plan's living document.
 *
 * NOTE the api's own behaviour: `GET` here is find-or-CREATE — it seeds the
 * document from the agent's plan template when one is bound. So a read can
 * have a side effect, which is the api's design rather than this client's, and
 * is worth knowing before calling it speculatively. It is called when someone
 * opens a plan, and not before.
 */
suspend fun readPlanDoc(
    client: HttpClient,
    instance: Instance,
    credential: Credential,
    planId: String,
): CommsResult<Artifact> =
    guardedCall(
        instance,
        call = { client.get("${instance.origin}${planDocPath(planId)}") { credential.applyTo(this) } },
        decode = { it.body<ArtifactEnvelope>().artifact },
    )
