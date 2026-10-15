package app.talaria.mobile

import androidx.compose.ui.test.*
import app.talaria.mobile.net.CommsResult
import app.talaria.mobile.screens.TAG_NEEDS_YOU
import app.talaria.mobile.screens.TAG_PLANS_EMPTY
import app.talaria.mobile.screens.TAG_PLAN_FAILED
import app.talaria.mobile.screens.TAG_PLAN_ROW
import app.talaria.mobile.screens.TAG_PLAN_WORKING
import app.talaria.mobile.screens.TAG_READ
import app.talaria.mobile.screens.TAG_READ_BACK
import app.talaria.mobile.screens.TAG_READ_BODY
import app.talaria.mobile.screens.TAG_READ_HIT
import app.talaria.mobile.screens.TAG_READ_NO_HITS
import app.talaria.mobile.screens.TAG_READ_QUERY
import app.talaria.mobile.screens.TAG_READ_TITLE
import app.talaria.mobile.screens.tabTag
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertTrue

/**
 * Reading, driven headlessly: find the paragraph that explains the ticket.
 */
@OptIn(ExperimentalTestApi::class)
class ReadUiTest {
    private fun ComposeUiTest.openRead() {
        waitUntilExactlyOneExists(hasTestTag(TAG_NEEDS_YOU))
        onNodeWithTag(tabTag(SignedTab.Read)).performClick()
        waitUntilExactlyOneExists(hasTestTag(TAG_READ))
    }

    /** Plans are listed on arrival, because they are the short list; search is
     *  the long one and waits to be asked. */
    @Test
    fun plans_are_listed_on_arrival() =
        runComposeUiTest {
            setContent { App(FakePorts().ports(), testStore(signedIn = true)) }
            openRead()

            waitUntilAtLeastOneExists(hasTestTag(TAG_PLAN_ROW))
            onAllNodesWithTag(TAG_PLAN_ROW).assertCountEquals(2)
            onNodeWithText("Q4 pricing").assertExists()
            // A plan with no title still reads as something.
            onNodeWithText("Untitled plan").assertExists()
        }

    @Test
    fun a_working_plan_and_a_failed_one_are_marked() =
        runComposeUiTest {
            setContent { App(FakePorts().ports(), testStore(signedIn = true)) }
            openRead()

            // useUnmergedTree: these markers live inside a clickable Card,
            // which merges its descendants' semantics — the third time that
            // has come up, and the reason docs/MOBILE.md names it.
            waitUntilAtLeastOneExists(hasTestTag(TAG_PLAN_ROW))
            onAllNodesWithTag(TAG_PLAN_WORKING, useUnmergedTree = true).assertCountEquals(1)
            onAllNodesWithTag(TAG_PLAN_FAILED, useUnmergedTree = true).assertCountEquals(1)
        }

    /** A search with no query is not a search — and firing one per keystroke
     *  would full-text-search "t", "th", "the". */
    @Test
    fun searching_requires_being_asked() =
        runComposeUiTest {
            val ports = FakePorts()
            setContent { App(ports.ports(), testStore(signedIn = true)) }
            openRead()

            onNodeWithTag(TAG_READ_QUERY).performTextInput("migration")
            assertTrue(ports.searched.isEmpty(), "typing is not searching")

            onNodeWithText("Search").performClick()
            waitUntil { ports.searched.isNotEmpty() }
            assertEquals(listOf("migration"), ports.searched)
        }

    /**
     * The catch this whole surface turns on: a snippet is pre-escaped HTML with
     * `<b>` highlights. Drawn raw it would show a reader literal tags and
     * entities.
     */
    @Test
    fun a_snippet_renders_as_text_with_no_markup_showing() =
        runComposeUiTest {
            setContent { App(FakePorts().ports(), testStore(signedIn = true)) }
            openRead()

            onNodeWithTag(TAG_READ_QUERY).performTextInput("migration")
            onNodeWithText("Search").performClick()
            waitUntilAtLeastOneExists(hasTestTag(TAG_READ_HIT))

            // The highlight and the entity are both resolved.
            onNodeWithText("run the migration before the deploy & watch the lock").assertExists()
            // And no markup survived anywhere on screen.
            onAllNodesWithText("<b>", substring = true).assertCountEquals(0)
            onAllNodesWithText("&amp;", substring = true).assertCountEquals(0)
        }

    @Test
    fun a_search_with_nothing_matching_says_so() =
        runComposeUiTest {
            val ports = FakePorts().apply { onSearchKnowledge = { _, _, _ -> CommsResult.Ok(emptyList()) } }
            setContent { App(ports.ports(), testStore(signedIn = true)) }
            openRead()

            onNodeWithTag(TAG_READ_QUERY).performTextInput("nothing")
            onNodeWithText("Search").performClick()
            waitUntilExactlyOneExists(hasTestTag(TAG_READ_NO_HITS))
        }

    @Test
    fun opening_a_hit_shows_the_document() =
        runComposeUiTest {
            val ports = FakePorts()
            setContent { App(ports.ports(), testStore(signedIn = true)) }
            openRead()

            onNodeWithTag(TAG_READ_QUERY).performTextInput("migration")
            onNodeWithText("Search").performClick()
            waitUntilAtLeastOneExists(hasTestTag(TAG_READ_HIT))
            onAllNodesWithTag(TAG_READ_HIT).onFirst().performClick()

            waitUntilExactlyOneExists(hasTestTag(TAG_READ_BODY))
            assertEquals(listOf("d1"), ports.docsOpened)
            onNodeWithTag(TAG_READ_TITLE).assertTextEquals("Deploy runbook")
            // The markdown subset at work: the heading, the bold-stripped
            // sentence, and the bullets.
            onNodeWithText("Run the migration first.").assertExists()
            onNodeWithText("check the lock").assertExists()
        }

    @Test
    fun opening_a_plan_shows_its_living_document() =
        runComposeUiTest {
            val ports = FakePorts()
            setContent { App(ports.ports(), testStore(signedIn = true)) }
            openRead()

            waitUntilAtLeastOneExists(hasTestTag(TAG_PLAN_ROW))
            onAllNodesWithTag(TAG_PLAN_ROW).onFirst().performClick()

            waitUntilExactlyOneExists(hasTestTag(TAG_READ_BODY))
            assertEquals(listOf("p1"), ports.planDocsOpened)
            onNodeWithTag(TAG_READ_TITLE).assertTextEquals("Q4 pricing plan")
            onNodeWithText("Goals").assertExists()
            onNodeWithText("decide the tiers").assertExists()
        }

    @Test
    fun going_back_returns_to_the_list() =
        runComposeUiTest {
            setContent { App(FakePorts().ports(), testStore(signedIn = true)) }
            openRead()

            waitUntilAtLeastOneExists(hasTestTag(TAG_PLAN_ROW))
            onAllNodesWithTag(TAG_PLAN_ROW).onFirst().performClick()
            waitUntilExactlyOneExists(hasTestTag(TAG_READ_BACK))

            onNodeWithTag(TAG_READ_BACK).performClick()
            waitUntilExactlyOneExists(hasTestTag(TAG_READ_QUERY))
            onNodeWithTag(TAG_READ_BODY).assertDoesNotExist()
        }

    @Test
    fun an_instance_with_no_plans_points_at_search_instead() =
        runComposeUiTest {
            val ports = FakePorts().apply { onPlans = { _, _ -> CommsResult.Ok(emptyList()) } }
            setContent { App(ports.ports(), testStore(signedIn = true)) }
            openRead()

            waitUntilExactlyOneExists(hasTestTag(TAG_PLANS_EMPTY))
            onNodeWithTag(TAG_READ_QUERY).assertExists()
        }

    @Test
    fun an_expired_session_while_reading_signs_the_account_out() =
        runComposeUiTest {
            val ports = FakePorts().apply { onPlans = { _, _ -> CommsResult.Expired } }
            val store = testStore(signedIn = true)
            setContent { App(ports.ports(), store) }

            waitUntilExactlyOneExists(hasTestTag(TAG_NEEDS_YOU))
            onNodeWithTag(tabTag(SignedTab.Read)).performClick()

            waitUntilExactlyOneExists(hasTestTag(Tags.INSTANCE_URL))
            assertTrue(store.accounts().isEmpty())
        }
}
