package app.talaria.mobile.net

import kotlinx.serialization.Serializable
import kotlinx.serialization.json.Json
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFalse
import kotlin.test.assertNull
import kotlin.test.assertTrue

/**
 * The fleet wire contract, which is two shapes joined on department.
 *
 * `GET /api/fleet/defs` answers a projection built by hand in
 * `talaria-agent-defs` (camelCase keys, plus `endpoints`, `brains` and
 * `canHire` this client does not model), and `GET /api/fleet/containers`
 * answers `talaria-fleet-docker`'s `AgentContainers` — which carries no
 * `rename_all` because every field is one word.
 */
@Serializable
private data class DefsWrapper(val defs: List<AgentDef> = emptyList())

@Serializable
private data class ContainersWrapper(val containers: List<AgentContainers> = emptyList())

class FleetParseTest {
    private val json = Json { ignoreUnknownKeys = true; explicitNulls = false }

    private val defsPayload = """
        {
          "defs":[
            {"id":"a1","slug":"sales","department":"sales","model":"claude-opus-5",
             "displayName":"Sales","role":"closer","emailAlias":"sales@x.com",
             "ownerUserId":"u1","enabled":true,"managed":true,"source":"muse",
             "developer":false,"ticketTemplateId":null,"planTemplateId":null,
             "currentVersion":3,"createdAt":"2026-09-01T10:00:00Z",
             "versions":[{"id":"v1","agentId":"a1","version":3}]}
          ],
          "endpoints":[{"id":"e1","price":3}],
          "brains":[{"agent":"sales"}],
          "canHire":true
        }
        """

    private val containersPayload = """
        {"containers":[
          {"department":"sales","managed":{"name":"talaria-fleet-sales-1","state":"running",
            "status":"Up 2 hours (healthy)","health":"healthy"}},
          {"department":"engineering","managed":{"name":"talaria-fleet-eng-1","state":"running",
            "status":"Up 5 minutes (unhealthy)","health":"unhealthy"}},
          {"department":"content","managed":null}
        ]}
        """

    @Test
    fun the_def_list_decodes_and_drops_what_is_not_modelled() {
        val defs = json.decodeFromString<DefsWrapper>(defsPayload).defs
        assertEquals(1, defs.size)
        val def = defs.single()
        assertEquals("a1", def.id)
        assertEquals("Sales", def.displayName)
        assertEquals("sales", def.department)
        assertTrue(def.managed)
        assertTrue(def.enabled)
        // endpoints, brains, canHire, versions and six def fields were all on
        // the wire and are all unmodelled. Decoding must survive it.
    }

    /** A def with no display name still has to render as something. */
    @Test
    fun an_agent_is_shown_by_the_best_handle_it_has() {
        assertEquals("Sales", json.decodeFromString<AgentDef>("""{"id":"a1","displayName":"Sales","slug":"sales"}""").shown)
        assertEquals("sales", json.decodeFromString<AgentDef>("""{"id":"a1","slug":"sales"}""").shown)
        assertEquals("a1", json.decodeFromString<AgentDef>("""{"id":"a1"}""").shown)
    }

    @Test
    fun the_container_list_decodes_including_a_department_with_nothing_running() {
        val containers = json.decodeFromString<ContainersWrapper>(containersPayload).containers
        assertEquals(3, containers.size)
        assertEquals("running", containers[0].managed?.state)
        assertEquals("healthy", containers[0].managed?.health)
        // A department with a null `managed` is a real answer: nothing is
        // running there.
        assertNull(containers[2].managed)
    }

    /**
     * The join, and the states it has to tell apart. "Running" alone would
     * call an unhealthy agent fine, and a missing healthcheck is not the same
     * as a failing one.
     */
    @Test
    fun the_join_distinguishes_running_unhealthy_starting_and_stopped() {
        val containers =
            json.decodeFromString<ContainersWrapper>(containersPayload)
                .containers
                .associateBy { it.department }

        fun agent(
            department: String,
            managed: Boolean = true,
        ) = FleetAgent(
            def = AgentDef(id = "x", department = department, managed = managed),
            container = containers[department]?.managed,
        )

        val healthy = agent("sales")
        assertTrue(healthy.isRunning)
        assertFalse(healthy.isUnhealthy)

        val unhealthy = agent("engineering")
        assertTrue(unhealthy.isRunning, "an unhealthy container is still running")
        assertTrue(unhealthy.isUnhealthy)

        // A department with a null container, and a department the containers
        // read never mentioned, are both stopped.
        assertFalse(agent("content").isRunning)
        assertFalse(agent("never-heard-of-it").isRunning)
    }

    /** No healthcheck at all is not unhealthy — an image may simply declare
     *  none, and calling that a failure would cry wolf on a healthy fleet. */
    @Test
    fun a_container_with_no_health_field_is_not_unhealthy() {
        val state = json.decodeFromString<ContainerState>("""{"name":"c","state":"running","status":"Up 1 hour"}""")
        assertNull(state.health)
        val agent = FleetAgent(def = AgentDef(id = "x", managed = true), container = state)
        assertTrue(agent.isRunning)
        assertFalse(agent.isUnhealthy)
        assertFalse(agent.isStarting)
    }

    @Test
    fun a_starting_container_is_running_but_not_yet_healthy() {
        val state =
            json.decodeFromString<ContainerState>(
                """{"name":"c","state":"running","status":"Up 2 seconds (health: starting)","health":"starting"}""",
            )
        val agent = FleetAgent(def = AgentDef(id = "x", managed = true), container = state)
        assertTrue(agent.isStarting)
        assertFalse(agent.isUnhealthy)
    }

    /** The three actions a phone sends, spelled the api's way. The other four
     *  are deliberately absent from the enum, so there is no way to send them
     *  by accident. */
    @Test
    fun the_action_wire_names_match_the_apis_enum() {
        assertEquals("up", AgentAction.Up.wire)
        assertEquals("stop", AgentAction.Stop.wire)
        assertEquals("restart", AgentAction.Restart.wire)
        assertEquals(3, AgentAction.entries.size, "retire/unretire/delete/roll are desk actions")
    }

    @Test
    fun an_empty_fleet_decodes_to_empty_lists() {
        assertTrue(json.decodeFromString<DefsWrapper>("""{"defs":[]}""").defs.isEmpty())
        assertTrue(json.decodeFromString<ContainersWrapper>("""{}""").containers.isEmpty())
    }
}
