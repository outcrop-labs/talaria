package app.talaria.mobile.net

import app.talaria.mobile.account.Credential
import app.talaria.mobile.account.Instance
import io.ktor.client.HttpClient
import io.ktor.client.call.body
import io.ktor.client.request.get
import io.ktor.client.request.post
import io.ktor.client.request.setBody
import io.ktor.http.ContentType
import io.ktor.http.contentType
import kotlinx.serialization.SerialName
import kotlinx.serialization.Serializable

/**
 * The fleet: which agents are up, and the switch that stops one.
 *
 * "Controller" taken literally. This is the surface that justifies the word —
 * an agent misbehaving at 11pm is a thing a person needs to be able to stop
 * from wherever they are, and every other way to do it is at a desk.
 *
 * TWO READS, JOINED ON DEPARTMENT, because that is how the api models it:
 * `GET /api/fleet/defs` lists the agents (`id`, `slug`, `displayName`,
 * `department`, `enabled`, `managed`), and `GET /api/fleet/containers` reports
 * container reality keyed by **department** rather than by agent. Control takes
 * an agent **id**. So the roster needs both, and an agent whose department has
 * no container row is stopped rather than unknown.
 *
 * Shapes from `talaria-agent-defs` (the `defs` projection, camelCase) and
 * `talaria-fleet-docker` (`AgentContainers` / `ContainerState` / `Health`,
 * which carry no rename because every field is one word).
 */
@Serializable
data class AgentDef(
    val id: String,
    val slug: String = "",
    @SerialName("displayName") val displayName: String = "",
    val department: String = "",
    val model: String = "",
    val role: String? = null,
    val enabled: Boolean = true,
    /** Talaria runs this one as a managed service. An unmanaged agent has no
     *  container to start or stop, so it gets no buttons. */
    val managed: Boolean = false,
) {
    val shown: String get() = displayName.takeIf { it.isNotBlank() } ?: slug.takeIf { it.isNotBlank() } ?: id
}

@Serializable
data class ContainerState(
    val name: String = "",
    /** `running`, `exited`, … — docker's own word. */
    val state: String = "",
    /** The human string including health, e.g. "Up 2 hours (healthy)". */
    val status: String = "",
    /** `starting` | `healthy` | `unhealthy`, or absent when the image declares
     *  no healthcheck — which is not the same as unhealthy. */
    val health: String? = null,
)

@Serializable
data class AgentContainers(
    val department: String = "",
    val managed: ContainerState? = null,
)

@Serializable
private data class DefsEnvelope(val defs: List<AgentDef> = emptyList())

@Serializable
private data class ContainersEnvelope(val containers: List<AgentContainers> = emptyList())

@Serializable
private data class ControlBody(val action: String)

/**
 * The lifecycle actions a phone offers, which is deliberately three of the
 * api's seven.
 *
 * `up`, `stop` and `restart` are reversible in one tap and fit in a sentence —
 * the scope line's test. `retire`, `unretire` and `delete` change what the
 * organisation *has* rather than what it is *doing*, and `roll` is a deploy;
 * all four belong at a desk with the diff in front of you.
 */
enum class AgentAction(val wire: String) {
    Up("up"),
    Stop("stop"),
    Restart("restart"),
}

/** One row of the roster: the agent, and whether its department is running. */
data class FleetAgent(
    val def: AgentDef,
    val container: ContainerState?,
) {
    val isRunning: Boolean get() = container?.state == "running"

    /** Up but failing its healthcheck — the state most worth surfacing, because
     *  "running" alone would call it fine. */
    val isUnhealthy: Boolean get() = isRunning && container?.health == "unhealthy"

    val isStarting: Boolean get() = isRunning && container?.health == "starting"
}

const val FLEET_DEFS_PATH = "/api/fleet/defs"
const val FLEET_CONTAINERS_PATH = "/api/fleet/containers"

fun agentControlPath(agentId: String) = "/api/fleet/agents/$agentId/control"

/**
 * The roster, joined.
 *
 * Containers are fetched second and failure there is NOT fatal: an agent list
 * with unknown run state is still useful, while no list at all is useless. The
 * api itself is careful about this read — it exists so a manager's tiles do not
 * render every agent as stopped — so the client should not throw that away over
 * a docker hiccup.
 */
suspend fun readFleet(
    client: HttpClient,
    instance: Instance,
    credential: Credential,
): CommsResult<List<FleetAgent>> {
    // `CommsResult` is covariant, so the failure branches already ARE a
    // `CommsResult<List<FleetAgent>>` and can be returned as they stand — no
    // cast, checked or otherwise.
    val defs =
        when (
            val result =
                guardedCall(
                    instance,
                    call = { client.get("${instance.origin}$FLEET_DEFS_PATH") { credential.applyTo(this) } },
                    decode = { it.body<DefsEnvelope>().defs },
                )
        ) {
            is CommsResult.Ok -> result.value
            is CommsResult.Failed -> return result
            CommsResult.Expired -> return CommsResult.Expired
        }

    val containers =
        guardedCall(
            instance,
            call = { client.get("${instance.origin}$FLEET_CONTAINERS_PATH") { credential.applyTo(this) } },
            decode = { it.body<ContainersEnvelope>().containers },
        )
    // An expired session must still propagate — that is not a hiccup.
    if (containers is CommsResult.Expired) return CommsResult.Expired

    val byDepartment =
        (containers as? CommsResult.Ok)?.value?.associateBy { it.department } ?: emptyMap()

    return CommsResult.Ok(
        defs.map { def -> FleetAgent(def, byDepartment[def.department]?.managed) },
    )
}

suspend fun controlAgent(
    client: HttpClient,
    instance: Instance,
    credential: Credential,
    agentId: String,
    action: AgentAction,
): CommsResult<Unit> =
    guardedCall(
        instance,
        call = {
            client.post("${instance.origin}${agentControlPath(agentId)}") {
                credential.applyTo(this)
                contentType(ContentType.Application.Json)
                setBody(ControlBody(action.wire))
            }
        },
        decode = { },
    )

/**
 * Stop a runaway work session on a ticket.
 *
 * Belongs with fleet control rather than with the ticket screen because it is
 * the same kind of act: something is running that should not be, and a person
 * is ending it.
 */
fun workSessionStopPath(taskId: String) = "/api/tasks/$taskId/work-session/stop"

suspend fun stopWorkSession(
    client: HttpClient,
    instance: Instance,
    credential: Credential,
    taskId: String,
): CommsResult<Unit> =
    guardedCall(
        instance,
        call = { client.post("${instance.origin}${workSessionStopPath(taskId)}") { credential.applyTo(this) } },
        decode = { },
    )
