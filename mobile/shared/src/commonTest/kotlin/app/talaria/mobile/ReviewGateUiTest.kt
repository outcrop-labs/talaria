package app.talaria.mobile

import androidx.compose.ui.test.*
import androidx.compose.ui.test.v2.runComposeUiTest
import app.talaria.mobile.net.CommsResult
import app.talaria.mobile.net.ReviewVerdict
import app.talaria.mobile.screens.TAG_APPROVE
import app.talaria.mobile.screens.TAG_APPROVED_MARK
import app.talaria.mobile.screens.TAG_COMMENT
import app.talaria.mobile.screens.TAG_COMMENT_BOX
import app.talaria.mobile.screens.TAG_COMMENT_SEND
import app.talaria.mobile.screens.TAG_GATE
import app.talaria.mobile.screens.TAG_NEEDS_YOU
import app.talaria.mobile.screens.TAG_REJECT
import app.talaria.mobile.screens.TAG_REVIEW_NOTES
import app.talaria.mobile.screens.TAG_TICKET
import app.talaria.mobile.screens.TAG_TICKET_BACK
import app.talaria.mobile.screens.TAG_TICKET_ERROR
import app.talaria.mobile.screens.TAG_TICKET_STATUS
import app.talaria.mobile.screens.TAG_TICKET_TITLE
import app.talaria.mobile.screens.TAG_WORK_ITEM
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertNull
import kotlin.test.assertTrue

/**
 * The five-second decision, driven headlessly: open a queue item, read enough
 * to decide, and say yes or no.
 *
 * The behaviour these pin down is the api's, not the obvious one — approving
 * MARKS a ticket signed off and leaves its column alone, rejecting is what
 * moves it. A screen that got that backwards would claim work had shipped when
 * it had not, which is the exact failure the gate exists to prevent.
 */
@OptIn(ExperimentalTestApi::class)
class ReviewGateUiTest {
    private fun ComposeUiTest.openFirstQueueItem() {
        waitUntilExactlyOneExists(hasTestTag(TAG_NEEDS_YOU))
        onAllNodesWithTag(TAG_WORK_ITEM).onFirst().performClick()
        waitUntilExactlyOneExists(hasTestTag(TAG_TICKET_TITLE))
    }

    /** A decision queue you cannot act from is just a list. */
    @Test
    fun a_queue_item_opens_its_ticket() =
        runComposeUiTest {
            val ports = FakePorts()
            setContent { App(ports.ports(), testStore(signedIn = true)) }
            openFirstQueueItem()

            // The same label rule as the queue, so one ticket reads the same
            // in both places.
            onNodeWithTag(TAG_TICKET_TITLE).assertTextEquals("TASK-12 · Fix the login loop")
            onNodeWithTag(TAG_TICKET_STATUS).assertTextEquals("quality_review")
            assertEquals(listOf("t1"), ports.ticketsRead)
        }

    /** Enough context to decide, which is the whole bar for this screen. */
    @Test
    fun the_ticket_shows_what_a_decision_needs() =
        runComposeUiTest {
            setContent { App(FakePorts().ports(), testStore(signedIn = true)) }
            openFirstQueueItem()

            onNodeWithText("Members on a stale session bounce between /login and /home.").assertExists()
            // An agent's own account of how it went leads rather than hiding
            // under the comments.
            onNodeWithText("outcome: Session refresh added; suite green.").assertExists()
            onNodeWithTag(TAG_COMMENT).assertExists()
        }

    @Test
    fun approving_sends_the_verdict_with_its_notes() =
        runComposeUiTest {
            val ports = FakePorts()
            setContent { App(ports.ports(), testStore(signedIn = true)) }
            openFirstQueueItem()

            onNodeWithTag(TAG_REVIEW_NOTES).performTextInput("read it, looks right")
            onNodeWithTag(TAG_APPROVE).performClick()

            waitUntil { ports.reviewed.isNotEmpty() }
            assertEquals(ReviewVerdict.Approve to "read it, looks right", ports.reviewed.single())
        }

    /** Notes are optional for an approval, and an empty box must not travel as
     *  an empty string pretending to be a note. */
    @Test
    fun approving_with_no_notes_sends_none() =
        runComposeUiTest {
            val ports = FakePorts()
            setContent { App(ports.ports(), testStore(signedIn = true)) }
            openFirstQueueItem()

            onNodeWithTag(TAG_APPROVE).performClick()
            waitUntil { ports.reviewed.isNotEmpty() }
            assertEquals(ReviewVerdict.Approve, ports.reviewed.single().first)
            assertEquals("", ports.reviewed.single().second)
        }

    /**
     * The one that matters most. Approving is a MARK: the ticket keeps its
     * column. If this ever asserts a moved status, the screen has started
     * claiming work shipped when it had not.
     */
    @Test
    fun approving_marks_the_ticket_and_does_not_move_it() =
        runComposeUiTest {
            val ports = FakePorts()
            setContent { App(ports.ports(), testStore(signedIn = true)) }
            openFirstQueueItem()

            onNodeWithTag(TAG_APPROVE).performClick()
            waitUntilExactlyOneExists(hasTestTag(TAG_APPROVED_MARK))

            // Still in quality_review — the mark changed, the column did not.
            onNodeWithTag(TAG_TICKET_STATUS).assertTextEquals("quality_review")
            assertTrue(ports.ticket.isApproved)
        }

    /** And once it is signed off the gate is gone: offering it again would
     *  invite approving the same work twice and make the queue lie. */
    @Test
    fun a_signed_off_ticket_is_not_offered_the_gate_again() =
        runComposeUiTest {
            val ports = FakePorts().apply { ticket = sampleTicket(approvedAt = "2026-10-14T10:00:00Z") }
            setContent { App(ports.ports(), testStore(signedIn = true)) }
            openFirstQueueItem()

            onNodeWithTag(TAG_APPROVED_MARK).assertExists()
            onNodeWithTag(TAG_GATE).assertDoesNotExist()
            onNodeWithTag(TAG_APPROVE).assertDoesNotExist()
            onNodeWithTag(TAG_REJECT).assertDoesNotExist()
        }

    /** Rejection IS the move — back to the board's first working column. */
    @Test
    fun sending_it_back_moves_it_and_leaves_it_unmarked() =
        runComposeUiTest {
            val ports = FakePorts()
            setContent { App(ports.ports(), testStore(signedIn = true)) }
            openFirstQueueItem()

            onNodeWithTag(TAG_REVIEW_NOTES).performTextInput("the second claim has no citation")
            onNodeWithTag(TAG_REJECT).performClick()

            waitUntil { ports.reviewed.isNotEmpty() }
            assertEquals(ReviewVerdict.Reject, ports.reviewed.single().first)
            assertEquals("the second claim has no citation", ports.reviewed.single().second)
            waitUntilExactlyOneExists(hasTextExactly("in_progress"))
            assertNull(ports.ticket.approvedAt, "a rejection must not mark the ticket approved")
        }

    /** The screen re-reads after a verdict rather than trusting its own guess
     *  about what the server did. */
    @Test
    fun a_verdict_re_reads_the_ticket() =
        runComposeUiTest {
            val ports = FakePorts()
            setContent { App(ports.ports(), testStore(signedIn = true)) }
            openFirstQueueItem()
            assertEquals(1, ports.ticketsRead.size)

            onNodeWithTag(TAG_APPROVE).performClick()
            waitUntilExactlyOneExists(hasTestTag(TAG_APPROVED_MARK))
            assertTrue(ports.ticketsRead.size >= 2)
        }

    /** A refused verdict says why and leaves the gate open — the board's
     *  owner/editor check is a 403 somebody has to be told about. */
    @Test
    fun a_refused_verdict_says_why_and_keeps_the_gate() =
        runComposeUiTest {
            val ports =
                FakePorts().apply {
                    onReview = { _, _, _, _, _ -> CommsResult.Failed("Outcrop Labs answered 403") }
                }
            setContent { App(ports.ports(), testStore(signedIn = true)) }
            openFirstQueueItem()

            onNodeWithTag(TAG_APPROVE).performClick()
            waitUntilExactlyOneExists(hasTestTag(TAG_TICKET_ERROR))
            onNodeWithTag(TAG_TICKET_ERROR).assertTextContains("403", substring = true)
            onNodeWithTag(TAG_GATE).assertExists()
        }

    @Test
    fun commenting_posts_and_shows_the_line() =
        runComposeUiTest {
            val ports = FakePorts()
            setContent { App(ports.ports(), testStore(signedIn = true)) }
            openFirstQueueItem()

            onNodeWithTag(TAG_COMMENT_BOX).performTextInput("which claim?")
            onNodeWithTag(TAG_COMMENT_SEND).performClick()

            waitUntil { ports.commented.isNotEmpty() }
            assertEquals("t1" to "which claim?", ports.commented.single())
            waitUntilExactlyOneExists(hasTextExactly("which claim?"))
        }

    @Test
    fun the_comment_button_is_dead_on_an_empty_box() =
        runComposeUiTest {
            setContent { App(FakePorts().ports(), testStore(signedIn = true)) }
            openFirstQueueItem()
            onNodeWithTag(TAG_COMMENT_SEND).assertIsNotEnabled()
        }

    /** A ticket overlays the tab it was opened from; back returns there. */
    @Test
    fun going_back_returns_to_the_queue() =
        runComposeUiTest {
            setContent { App(FakePorts().ports(), testStore(signedIn = true)) }
            openFirstQueueItem()

            onNodeWithTag(TAG_TICKET_BACK).performClick()
            waitUntilExactlyOneExists(hasTestTag(TAG_NEEDS_YOU))
            onNodeWithTag(TAG_TICKET).assertDoesNotExist()
        }

    @Test
    fun an_expired_session_on_a_ticket_signs_the_account_out() =
        runComposeUiTest {
            val ports = FakePorts().apply { onTask = { _, _, _ -> CommsResult.Expired } }
            val store = testStore(signedIn = true)
            setContent { App(ports.ports(), store) }

            waitUntilExactlyOneExists(hasTestTag(TAG_NEEDS_YOU))
            onAllNodesWithTag(TAG_WORK_ITEM).onFirst().performClick()

            waitUntilExactlyOneExists(hasTestTag(Tags.INSTANCE_URL))
            assertTrue(store.accounts().isEmpty())
        }
}
