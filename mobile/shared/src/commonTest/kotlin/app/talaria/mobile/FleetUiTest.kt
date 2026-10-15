package app.talaria.mobile

import androidx.compose.ui.test.*
import androidx.compose.ui.test.v2.runComposeUiTest
import app.talaria.mobile.net.AgentAction
import app.talaria.mobile.net.AgentDef
import app.talaria.mobile.net.CommsResult
import app.talaria.mobile.net.ContainerState
import app.talaria.mobile.net.FleetAgent
import app.talaria.mobile.screens.TAG_FLEET
import app.talaria.mobile.screens.TAG_FLEET_DOT
import app.talaria.mobile.screens.TAG_FLEET_EMPTY
import app.talaria.mobile.screens.TAG_FLEET_ERROR
import app.talaria.mobile.screens.TAG_FLEET_RESTART
import app.talaria.mobile.screens.TAG_FLEET_ROW
import app.talaria.mobile.screens.TAG_FLEET_START
import app.talaria.mobile.screens.TAG_FLEET_STOP
import app.talaria.mobile.screens.TAG_FLEET_UNHEALTHY
import app.talaria.mobile.screens.TAG_FLEET_UNMANAGED
import app.talaria.mobile.screens.TAG_NEEDS_YOU
import app.talaria.mobile.screens.tabTag
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertTrue

/**
 * The kill switch, driven headlessly.
 *
 * The behaviour worth pinning is which buttons exist: a running agent can be
 * stopped or restarted, a stopped one started, and an agent Talaria does not
 * run as a service gets no buttons at all — offering one that cannot work is
 * worse than offering none.
 */
@OptIn(ExperimentalTestApi::class)
class FleetUiTest {
    private fun ComposeUiTest.openFleet() {
        waitUntilExactlyOneExists(hasTestTag(TAG_NEEDS_YOU))
        onNodeWithTag(tabTag(SignedTab.Fleet)).performClick()
        waitUntilExactlyOneExists(hasTestTag(TAG_FLEET))
    }

    @Test
    fun the_roster_shows_every_agent_and_its_container_state() =
        runComposeUiTest {
            setContent { App(FakePorts().ports(), testStore(signedIn = true)) }
            openFleet()

            waitUntilAtLeastOneExists(hasTestTag(TAG_FLEET_ROW))
            onAllNodesWithTag(TAG_FLEET_ROW).assertCountEquals(4)
            onNodeWithText("Sales").assertExists()
            onNodeWithText("Up 2 hours (healthy)").assertExists()
            // No container row at all means stopped, not unknown — and two of
            // the four are, so this is a count rather than a single node.
            onAllNodesWithText("stopped").assertCountEquals(2)
        }

    /** Trouble first. A roster sorted by name makes you hunt for the one thing
     *  that needs you, which is the opposite of a glance. */
    @Test
    fun the_unhealthy_agent_comes_first() =
        runComposeUiTest {
            setContent { App(FakePorts().ports(), testStore(signedIn = true)) }
            openFleet()

            waitUntilAtLeastOneExists(hasTestTag(TAG_FLEET_ROW))
            // Asserted structurally rather than by text: a Card merges its
            // descendants' semantics, so matching merged text says nothing
            // reliable about WHICH row matched. The unhealthy marker is only
            // rendered inside the unhealthy agent's row, so finding it under
            // the first row is the ordering claim stated exactly.
            onAllNodesWithTag(TAG_FLEET_ROW)
                .onFirst()
                .assert(hasAnyDescendant(hasTestTag(TAG_FLEET_UNHEALTHY)))
        }

    /** Running-but-failing is the state most worth surfacing, because
     *  "running" on its own would call it fine. */
    @Test
    fun an_unhealthy_agent_says_so_rather_than_just_running() =
        runComposeUiTest {
            setContent { App(FakePorts().ports(), testStore(signedIn = true)) }
            openFleet()

            waitUntilExactlyOneExists(hasTestTag(TAG_FLEET_UNHEALTHY))
            onNodeWithTag(TAG_FLEET_UNHEALTHY).assertTextContains("healthcheck", substring = true)
            // It is still running, so it is still stoppable.
            onAllNodesWithTag(TAG_FLEET_STOP).fetchSemanticsNodes().isNotEmpty()
        }

    @Test
    fun a_running_agent_can_be_stopped_and_restarted_but_not_started() =
        runComposeUiTest {
            val ports =
                FakePorts().apply {
                    onFleet = { _, _ ->
                        CommsResult.Ok(
                            listOf(
                                FleetAgent(
                                    def = AgentDef(id = "a1", displayName = "Sales", department = "sales", managed = true),
                                    container = ContainerState(state = "running", status = "Up 1 hour", health = "healthy"),
                                ),
                            ),
                        )
                    }
                }
            setContent { App(ports.ports(), testStore(signedIn = true)) }
            openFleet()

            waitUntilExactlyOneExists(hasTestTag(TAG_FLEET_STOP))
            onNodeWithTag(TAG_FLEET_RESTART).assertExists()
            onNodeWithTag(TAG_FLEET_START).assertDoesNotExist()
        }

    @Test
    fun a_stopped_agent_can_only_be_started() =
        runComposeUiTest {
            val ports =
                FakePorts().apply {
                    onFleet = { _, _ ->
                        CommsResult.Ok(
                            listOf(
                                FleetAgent(
                                    def = AgentDef(id = "a1", displayName = "Content", department = "content", managed = true),
                                    container = null,
                                ),
                            ),
                        )
                    }
                }
            setContent { App(ports.ports(), testStore(signedIn = true)) }
            openFleet()

            waitUntilExactlyOneExists(hasTestTag(TAG_FLEET_START))
            onNodeWithTag(TAG_FLEET_STOP).assertDoesNotExist()
            onNodeWithTag(TAG_FLEET_RESTART).assertDoesNotExist()
        }

    /** No container, no buttons. */
    @Test
    fun an_unmanaged_agent_gets_no_buttons_and_an_explanation() =
        runComposeUiTest {
            val ports =
                FakePorts().apply {
                    onFleet = { _, _ ->
                        CommsResult.Ok(
                            listOf(
                                FleetAgent(
                                    def = AgentDef(id = "a1", displayName = "External", department = "ext", managed = false),
                                    container = null,
                                ),
                            ),
                        )
                    }
                }
            setContent { App(ports.ports(), testStore(signedIn = true)) }
            openFleet()

            waitUntilExactlyOneExists(hasTestTag(TAG_FLEET_UNMANAGED))
            onNodeWithTag(TAG_FLEET_START).assertDoesNotExist()
            onNodeWithTag(TAG_FLEET_STOP).assertDoesNotExist()
        }

    @Test
    fun stopping_an_agent_sends_stop_for_that_agent() =
        runComposeUiTest {
            val ports = FakePorts()
            setContent { App(ports.ports(), testStore(signedIn = true)) }
            openFleet()

            waitUntilAtLeastOneExists(hasTestTag(TAG_FLEET_STOP))
            // Engineering is first (unhealthy), and it is running, so its row
            // carries the stop button.
            onAllNodesWithTag(TAG_FLEET_STOP).onFirst().performClick()

            waitUntil { ports.controlled.isNotEmpty() }
            assertEquals("a-eng" to AgentAction.Stop, ports.controlled.single())
        }

    @Test
    fun starting_a_stopped_agent_sends_up() =
        runComposeUiTest {
            val ports = FakePorts()
            setContent { App(ports.ports(), testStore(signedIn = true)) }
            openFleet()

            waitUntilAtLeastOneExists(hasTestTag(TAG_FLEET_START))
            onAllNodesWithTag(TAG_FLEET_START).onFirst().performClick()

            waitUntil { ports.controlled.isNotEmpty() }
            assertEquals(AgentAction.Up, ports.controlled.single().second)
        }

    /** Container reality changed, so the roster is re-read rather than guessed
     *  at from the action that was sent. */
    @Test
    fun a_control_action_re_reads_the_roster() =
        runComposeUiTest {
            val ports = FakePorts()
            var reads = 0
            ports.onFleet = { _, _ ->
                reads++
                CommsResult.Ok(sampleFleet())
            }
            setContent { App(ports.ports(), testStore(signedIn = true)) }
            openFleet()

            waitUntilAtLeastOneExists(hasTestTag(TAG_FLEET_STOP))
            assertEquals(1, reads)
            onAllNodesWithTag(TAG_FLEET_STOP).onFirst().performClick()
            waitUntil { reads >= 2 }
        }

    /** The board's permission check is a 403 somebody has to be told about —
     *  agent control is managers and admins only. */
    @Test
    fun a_refused_action_says_why() =
        runComposeUiTest {
            val ports =
                FakePorts().apply {
                    onControl = { _, _, _, _ -> CommsResult.Failed("Outcrop Labs answered 403") }
                }
            setContent { App(ports.ports(), testStore(signedIn = true)) }
            openFleet()

            waitUntilAtLeastOneExists(hasTestTag(TAG_FLEET_STOP))
            onAllNodesWithTag(TAG_FLEET_STOP).onFirst().performClick()

            waitUntilExactlyOneExists(hasTestTag(TAG_FLEET_ERROR))
            onNodeWithTag(TAG_FLEET_ERROR).assertTextContains("403", substring = true)
        }

    @Test
    fun an_instance_with_no_agents_says_so() =
        runComposeUiTest {
            val ports = FakePorts().apply { onFleet = { _, _ -> CommsResult.Ok(emptyList()) } }
            setContent { App(ports.ports(), testStore(signedIn = true)) }
            openFleet()
            waitUntilExactlyOneExists(hasTestTag(TAG_FLEET_EMPTY))
        }

    @Test
    fun every_row_carries_a_status_dot() =
        runComposeUiTest {
            setContent { App(FakePorts().ports(), testStore(signedIn = true)) }
            openFleet()
            waitUntilAtLeastOneExists(hasTestTag(TAG_FLEET_ROW))
            onAllNodesWithTag(TAG_FLEET_DOT, useUnmergedTree = true).assertCountEquals(4)
        }

    @Test
    fun an_expired_session_in_the_fleet_signs_the_account_out() =
        runComposeUiTest {
            val ports = FakePorts().apply { onFleet = { _, _ -> CommsResult.Expired } }
            val store = testStore(signedIn = true)
            setContent { App(ports.ports(), store) }

            waitUntilExactlyOneExists(hasTestTag(TAG_NEEDS_YOU))
            onNodeWithTag(tabTag(SignedTab.Fleet)).performClick()

            waitUntilExactlyOneExists(hasTestTag(Tags.INSTANCE_URL))
            assertTrue(store.accounts().isEmpty())
        }
}
