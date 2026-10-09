package app.talaria.mobile

import androidx.compose.ui.test.*
import app.talaria.mobile.net.CommsResult
import app.talaria.mobile.net.Message
import app.talaria.mobile.net.UserEvent
import app.talaria.mobile.screens.TAG_BACK_TO_LIST
import app.talaria.mobile.screens.TAG_CHANNEL_LIST
import app.talaria.mobile.screens.TAG_CHANNEL_ROW
import app.talaria.mobile.screens.TAG_CHANNEL_UNREAD
import app.talaria.mobile.screens.TAG_COMMS_EMPTY
import app.talaria.mobile.screens.TAG_COMPOSER
import app.talaria.mobile.screens.TAG_MESSAGE
import app.talaria.mobile.screens.TAG_NEEDS_YOU
import app.talaria.mobile.screens.TAG_SEND
import app.talaria.mobile.screens.tabTag
import app.talaria.mobile.SignedTab
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertTrue

/**
 * Comms, driven headlessly: find the room, read it, say something, and have
 * the room update itself when the firehose says it changed.
 */
@OptIn(ExperimentalTestApi::class)
class CommsUiTest {
    private fun ComposeUiTest.openComms() {
        waitUntilExactlyOneExists(hasTestTag(TAG_NEEDS_YOU))
        onNodeWithTag(tabTag(SignedTab.Comms)).performClick()
        waitUntilExactlyOneExists(hasTestTag(TAG_CHANNEL_LIST))
    }

    @Test
    fun channels_and_dms_are_one_list_labelled_the_way_people_name_them() =
        runComposeUiTest {
            setContent { App(FakePorts().ports(), testStore(signedIn = true)) }
            openComms()

            // A channel is "#name"; a DM is named by whoever is in it, because
            // "dm-7f3a" is not something anyone recognises.
            onNodeWithText("#platform").assertExists()
            onNodeWithText("#general").assertExists()
            onNodeWithText("Muse").assertExists()
            onNodeWithText("dm-7f3a").assertDoesNotExist()
        }

    /** Unread leads the sort — the only ordering a glance can use. */
    @Test
    fun rooms_with_something_new_come_first() =
        runComposeUiTest {
            setContent { App(FakePorts().ports(), testStore(signedIn = true)) }
            openComms()

            val rows = onAllNodesWithTag(TAG_CHANNEL_ROW)
            rows.assertCountEquals(3)
            // platform (3 unread) then the DM (1) then general (0).
            rows[0].assertTextContains("#platform", substring = true)
            rows[2].assertTextContains("#general", substring = true)
            // useUnmergedTree: a clickable Card merges its descendants'
            // semantics, so the badge is not addressable on its own in the
            // merged tree even though it is on screen.
            onAllNodesWithTag(TAG_CHANNEL_UNREAD, useUnmergedTree = true).assertCountEquals(2)
        }

    @Test
    fun an_instance_with_no_rooms_says_so() =
        runComposeUiTest {
            val ports = FakePorts().apply { onChannels = { _, _ -> CommsResult.Ok(emptyList()) } }
            setContent { App(ports.ports(), testStore(signedIn = true)) }

            waitUntilExactlyOneExists(hasTestTag(TAG_NEEDS_YOU))
            onNodeWithTag(tabTag(SignedTab.Comms)).performClick()
            waitUntilExactlyOneExists(hasTestTag(TAG_COMMS_EMPTY))
        }

    /** Opening a room is reading it: the badge should not survive the visit. */
    @Test
    fun opening_a_room_reads_its_messages_and_marks_it_read() =
        runComposeUiTest {
            val ports = FakePorts()
            setContent { App(ports.ports(), testStore(signedIn = true)) }
            openComms()

            onNodeWithText("#platform").performClick()
            waitUntilAtLeastOneExists(hasTestTag(TAG_MESSAGE))

            assertEquals(listOf("c-busy"), ports.messagesRead)
            assertEquals(listOf("c-busy"), ports.markedRead)
        }

    /** An agent is labelled as an agent. Talaria's premise is that agents are
     *  staff, which only works if you can tell which colleague is one. */
    @Test
    fun an_agents_message_is_marked_as_an_agents() =
        runComposeUiTest {
            setContent { App(FakePorts().ports(), testStore(signedIn = true)) }
            openComms()
            onNodeWithText("#platform").performClick()
            waitUntilAtLeastOneExists(hasTestTag(TAG_MESSAGE))

            onNodeWithText("Not yet — the judge flagged one claim.").assertExists()
            onNodeWithText("agent").assertExists()
        }

    /** Thread replies hang off their root; a phone shows the room, not the
     *  tree. The sample has three messages, one of them a reply. */
    @Test
    fun thread_replies_stay_out_of_the_main_flow() =
        runComposeUiTest {
            setContent { App(FakePorts().ports(), testStore(signedIn = true)) }
            openComms()
            onNodeWithText("#platform").performClick()
            waitUntilAtLeastOneExists(hasTestTag(TAG_MESSAGE))

            onAllNodesWithTag(TAG_MESSAGE).assertCountEquals(2)
            onNodeWithText("which claim?").assertDoesNotExist()
        }

    @Test
    fun sending_a_message_posts_it_clears_the_box_and_shows_the_line() =
        runComposeUiTest {
            val ports = FakePorts()
            setContent { App(ports.ports(), testStore(signedIn = true)) }
            openComms()
            onNodeWithText("#platform").performClick()
            waitUntilAtLeastOneExists(hasTestTag(TAG_MESSAGE))

            onNodeWithTag(TAG_COMPOSER).performTextInput("ship it")
            onNodeWithTag(TAG_SEND).performClick()

            waitUntil { ports.posted.isNotEmpty() }
            assertEquals(listOf("c-busy" to "ship it"), ports.posted)
            // The response's own line is appended rather than waiting for the
            // stream to echo it — a composer that clears before the message
            // appears feels broken.
            onNodeWithText("ship it").assertExists()
            onNodeWithTag(TAG_COMPOSER).assertTextContains("")
        }

    @Test
    fun the_send_button_is_dead_on_an_empty_box() =
        runComposeUiTest {
            setContent { App(FakePorts().ports(), testStore(signedIn = true)) }
            openComms()
            onNodeWithText("#platform").performClick()
            waitUntilAtLeastOneExists(hasTestTag(TAG_MESSAGE))

            onNodeWithTag(TAG_SEND).assertIsNotEnabled()
            onNodeWithTag(TAG_COMPOSER).performTextInput("   ")
            onNodeWithTag(TAG_SEND).assertIsNotEnabled()
        }

    /**
     * The realtime payoff, and the invariant made visible. A `channel` event
     * carries an id and nothing else, so the only correct response is to
     * re-read through the ordinary ACL'd route — which is why no event ever
     * needs to carry a message body.
     */
    @Test
    fun a_channel_event_makes_the_open_room_re_read_itself() =
        runComposeUiTest {
            val ports = FakePorts()
            var served = sampleMessages()
            ports.onMessages = { _, _, _ -> CommsResult.Ok(served) }

            setContent { App(ports.ports(), testStore(signedIn = true)) }
            openComms()
            onNodeWithText("#platform").performClick()
            waitUntilAtLeastOneExists(hasTestTag(TAG_MESSAGE))
            onNodeWithText("landed while you were reading").assertDoesNotExist()

            // The server announces a change; the body is NOT in the event.
            served = served + Message(
                id = "m4",
                seq = 4,
                authorType = "agent",
                author = "Muse",
                content = "landed while you were reading",
            )
            ports.eventBus.tryEmit(UserEvent.Channel("c-busy"))

            waitUntilExactlyOneExists(hasTextExactly("landed while you were reading"))
            assertTrue(ports.messagesRead.size >= 2, "the event must cause a re-read, not a guess")
        }

    /** Another room's event is the list's business, not this screen's. */
    @Test
    fun an_event_for_a_different_room_does_not_re_read_this_one() =
        runComposeUiTest {
            val ports = FakePorts()
            setContent { App(ports.ports(), testStore(signedIn = true)) }
            openComms()
            onNodeWithText("#platform").performClick()
            waitUntilAtLeastOneExists(hasTestTag(TAG_MESSAGE))
            val before = ports.messagesRead.size

            ports.eventBus.tryEmit(UserEvent.Channel("c-quiet"))
            ports.eventBus.tryEmit(UserEvent.Typing(channelId = "c-busy", userId = "u1", typing = true))

            // Typing is presence, not content: it must not trigger a fetch.
            waitUntil { true }
            assertEquals(before, ports.messagesRead.size)
        }

    @Test
    fun going_back_returns_to_the_room_list() =
        runComposeUiTest {
            setContent { App(FakePorts().ports(), testStore(signedIn = true)) }
            openComms()
            onNodeWithText("#platform").performClick()
            waitUntilAtLeastOneExists(hasTestTag(TAG_MESSAGE))

            onNodeWithTag(TAG_BACK_TO_LIST).performClick()
            waitUntilExactlyOneExists(hasTestTag(TAG_CHANNEL_LIST))
        }

    /** Switching back to Home closes the room: returning later to a channel
     *  you did not choose is disorienting. */
    @Test
    fun leaving_comms_closes_the_open_room() =
        runComposeUiTest {
            setContent { App(FakePorts().ports(), testStore(signedIn = true)) }
            openComms()
            onNodeWithText("#platform").performClick()
            waitUntilAtLeastOneExists(hasTestTag(TAG_MESSAGE))

            onNodeWithTag(tabTag(SignedTab.Home)).performClick()
            waitUntilExactlyOneExists(hasTestTag(TAG_NEEDS_YOU))
            onNodeWithTag(tabTag(SignedTab.Comms)).performClick()

            waitUntilExactlyOneExists(hasTestTag(TAG_CHANNEL_LIST))
        }

    @Test
    fun an_expired_session_in_comms_signs_the_account_out() =
        runComposeUiTest {
            val ports = FakePorts().apply { onChannels = { _, _ -> CommsResult.Expired } }
            val store = testStore(signedIn = true)
            setContent { App(ports.ports(), store) }

            waitUntilExactlyOneExists(hasTestTag(TAG_NEEDS_YOU))
            onNodeWithTag(tabTag(SignedTab.Comms)).performClick()

            waitUntilExactlyOneExists(hasTestTag(Tags.INSTANCE_URL))
            assertTrue(store.accounts().isEmpty())
        }
}
