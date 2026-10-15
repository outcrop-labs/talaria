package app.talaria.mobile

import androidx.compose.ui.test.*
import app.talaria.mobile.net.CommsResult
import app.talaria.mobile.net.CostResult
import app.talaria.mobile.net.WorkSessionRead
import app.talaria.mobile.screens.TAG_COST
import app.talaria.mobile.screens.TAG_COST_FORBIDDEN
import app.talaria.mobile.screens.TAG_FLEET
import app.talaria.mobile.screens.TAG_FLEET_ROW
import app.talaria.mobile.screens.TAG_NEEDS_YOU
import app.talaria.mobile.screens.TAG_SESSION
import app.talaria.mobile.screens.TAG_SESSION_STOP
import app.talaria.mobile.screens.TAG_SESSION_TAIL
import app.talaria.mobile.screens.TAG_TICKET_TITLE
import app.talaria.mobile.screens.TAG_WORK_ITEM
import app.talaria.mobile.screens.tabTag
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertTrue

/**
 * Watching: what an agent is doing on a ticket, and what the workforce costs.
 *
 * Both are information rather than action, so the behaviour worth pinning is
 * mostly about absence — a ticket with no run shows no ticker, and an account
 * without the Observability view is told that plainly rather than shown an
 * error.
 */
@OptIn(ExperimentalTestApi::class)
class WatchUiTest {
    private fun ComposeUiTest.openFirstTicket() {
        waitUntilExactlyOneExists(hasTestTag(TAG_NEEDS_YOU))
        onAllNodesWithTag(TAG_WORK_ITEM).onFirst().performClick()
        waitUntilExactlyOneExists(hasTestTag(TAG_TICKET_TITLE))
    }

    private fun ComposeUiTest.openFleet() {
        waitUntilExactlyOneExists(hasTestTag(TAG_NEEDS_YOU))
        onNodeWithTag(tabTag(SignedTab.Fleet)).performClick()
        waitUntilExactlyOneExists(hasTestTag(TAG_FLEET))
    }

    // ---- the live session on a ticket ------------------------------------

    @Test
    fun a_running_agent_shows_what_it_is_doing() =
        runComposeUiTest {
            val ports = FakePorts().apply { onWorkSession = { _, _, _ -> CommsResult.Ok(sampleSession()) } }
            setContent { App(ports.ports(), testStore(signedIn = true)) }
            openFirstTicket()

            waitUntilExactlyOneExists(hasTestTag(TAG_SESSION))
            // Model, phase and turn, so a glance says how far along it is.
            onNodeWithText("Muse · acting · turn 7").assertExists()
            // The tail is the one place an agent's own words arrive without a
            // second fetch — sound here, since it is this ticket's own run.
            onNodeWithTag(TAG_SESSION_TAIL).assertTextEquals("Re-ran the migration; it passes now.")
        }

    /** Nothing running is a real answer, not an absence to fill with a spinner. */
    @Test
    fun a_ticket_with_no_run_shows_no_ticker() =
        runComposeUiTest {
            val ports =
                FakePorts().apply {
                    onWorkSession = { _, _, _ -> CommsResult.Ok(WorkSessionRead(session = null)) }
                }
            setContent { App(ports.ports(), testStore(signedIn = true)) }
            openFirstTicket()

            onNodeWithTag(TAG_SESSION).assertDoesNotExist()
            onNodeWithTag(TAG_SESSION_STOP).assertDoesNotExist()
        }

    @Test
    fun stopping_a_run_sends_the_stop_and_re_reads() =
        runComposeUiTest {
            val ports = FakePorts()
            var reads = 0
            ports.onWorkSession = { _, _, _ ->
                reads++
                CommsResult.Ok(if (reads == 1) sampleSession() else WorkSessionRead(session = null))
            }
            setContent { App(ports.ports(), testStore(signedIn = true)) }
            openFirstTicket()

            waitUntilExactlyOneExists(hasTestTag(TAG_SESSION_STOP))
            onNodeWithTag(TAG_SESSION_STOP).performClick()

            waitUntil { ports.stoppedSessions.isNotEmpty() }
            assertEquals(listOf("t1"), ports.stoppedSessions)
            // The ticker goes once the run is gone, because the screen re-read
            // rather than assuming.
            waitUntil { reads >= 2 }
            onNodeWithTag(TAG_SESSION).assertDoesNotExist()
        }

    /** A session read that fails must not take the ticket with it: the gate is
     *  the point of the screen, and a docker hiccup is not a reason to hide it. */
    @Test
    fun a_failed_session_read_leaves_the_ticket_usable() =
        runComposeUiTest {
            val ports =
                FakePorts().apply {
                    onWorkSession = { _, _, _ -> CommsResult.Failed("could not reach Outcrop Labs") }
                }
            setContent { App(ports.ports(), testStore(signedIn = true)) }
            openFirstTicket()

            onNodeWithTag(TAG_SESSION).assertDoesNotExist()
            // The ticket, and its gate, are still there.
            onNodeWithTag(TAG_TICKET_TITLE).assertTextEquals("TASK-12 · Fix the login loop")
        }

    // ---- the cost glance -------------------------------------------------

    @Test
    fun the_ledger_shows_todays_spend_on_the_fleet_tab() =
        runComposeUiTest {
            val ports = FakePorts().apply { onCost = { _, _ -> sampleCost() } }
            setContent { App(ports.ports(), testStore(signedIn = true)) }
            openFleet()

            waitUntilExactlyOneExists(hasTestTag(TAG_COST))
            // 90k prompt + 30k completion = 120000 tokens, $4.19, 214 calls.
            onNodeWithTag(TAG_COST).assertTextEquals("today: $4.19 · 120000 tokens · 214 generations")
        }

    /** Money that renders as 4.199999 is money nobody trusts, and common Kotlin
     *  has no `%.2f` — so the rounding is hand-rolled and worth asserting. */
    @Test
    fun money_is_rendered_to_two_places() =
        runComposeUiTest {
            val ports = FakePorts().apply { onCost = { _, _ -> sampleCost(todayCost = 4.199999) } }
            setContent { App(ports.ports(), testStore(signedIn = true)) }
            openFleet()

            waitUntilExactlyOneExists(hasTestTag(TAG_COST))
            onNodeWithTag(TAG_COST).assertTextContains("$4.20", substring = true)
        }

    @Test
    fun a_whole_dollar_still_shows_two_places() =
        runComposeUiTest {
            val ports = FakePorts().apply { onCost = { _, _ -> sampleCost(todayCost = 3.0) } }
            setContent { App(ports.ports(), testStore(signedIn = true)) }
            openFleet()

            waitUntilExactlyOneExists(hasTestTag(TAG_COST))
            onNodeWithTag(TAG_COST).assertTextContains("$3.00", substring = true)
        }

    @Test
    fun a_quiet_day_shows_zero_rather_than_nothing() =
        runComposeUiTest {
            val ports =
                FakePorts().apply {
                    onCost = { _, _ -> sampleCost(todayCost = 0.0, todayPrompt = 0, todayCompletion = 0, generations = 0) }
                }
            setContent { App(ports.ports(), testStore(signedIn = true)) }
            openFleet()

            waitUntilExactlyOneExists(hasTestTag(TAG_COST))
            onNodeWithTag(TAG_COST).assertTextEquals("today: $0.00 · 0 tokens · 0 generations")
        }

    /**
     * The important one. Most members do not hold `view:/observability`, so a
     * 403 here is the permission model working — not a failure. Showing
     * "answered 403" to most of the workforce would be a bug with a very
     * confusing shape.
     */
    @Test
    fun an_account_without_the_view_is_told_plainly_not_shown_an_error() =
        runComposeUiTest {
            val ports = FakePorts() // Forbidden is the default
            setContent { App(ports.ports(), testStore(signedIn = true)) }
            openFleet()

            waitUntilExactlyOneExists(hasTestTag(TAG_COST_FORBIDDEN))
            onNodeWithTag(TAG_COST).assertDoesNotExist()
            // And the roster is unaffected: the ledger is a line, not the page.
            waitUntilAtLeastOneExists(hasTestTag(TAG_FLEET_ROW))
        }

    /** A ledger that cannot be read is simply absent — it is one line of
     *  information, and shouting about it would crowd out the roster. */
    @Test
    fun a_failed_ledger_read_is_silent() =
        runComposeUiTest {
            val ports = FakePorts().apply { onCost = { _, _ -> CostResult.Failed("could not reach Outcrop Labs") } }
            setContent { App(ports.ports(), testStore(signedIn = true)) }
            openFleet()

            waitUntilAtLeastOneExists(hasTestTag(TAG_FLEET_ROW))
            onNodeWithTag(TAG_COST).assertDoesNotExist()
            onNodeWithTag(TAG_COST_FORBIDDEN).assertDoesNotExist()
        }

    /** An expired session is the roster read's business — it signs the account
     *  out. The ledger must not race it into a second notice. */
    @Test
    fun an_expired_ledger_read_does_not_fight_the_roster() =
        runComposeUiTest {
            val ports =
                FakePorts().apply {
                    onCost = { _, _ -> CostResult.Expired }
                    onFleet = { _, _ -> CommsResult.Expired }
                }
            val store = testStore(signedIn = true)
            setContent { App(ports.ports(), store) }

            waitUntilExactlyOneExists(hasTestTag(TAG_NEEDS_YOU))
            onNodeWithTag(tabTag(SignedTab.Fleet)).performClick()

            waitUntilExactlyOneExists(hasTestTag(Tags.INSTANCE_URL))
            assertTrue(store.accounts().isEmpty())
        }
}
