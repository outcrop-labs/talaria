package app.talaria.mobile

import androidx.compose.ui.test.*
import app.talaria.mobile.net.ChatEvent
import app.talaria.mobile.net.ToolStatus
import app.talaria.mobile.screens.TAG_ASSISTANT
import app.talaria.mobile.screens.TAG_ASSISTANT_COMPOSER
import app.talaria.mobile.screens.TAG_ASSISTANT_ERROR
import app.talaria.mobile.screens.TAG_ASSISTANT_NEW
import app.talaria.mobile.screens.TAG_ASSISTANT_NO_MODEL
import app.talaria.mobile.screens.TAG_ASSISTANT_QUEUED
import app.talaria.mobile.screens.TAG_ASSISTANT_REASONING
import app.talaria.mobile.screens.TAG_ASSISTANT_SEND
import app.talaria.mobile.screens.TAG_ASSISTANT_TOOL
import app.talaria.mobile.screens.TAG_ASSISTANT_TURN
import app.talaria.mobile.screens.TAG_ASSISTANT_USAGE
import app.talaria.mobile.screens.TAG_NEEDS_YOU
import app.talaria.mobile.screens.tabTag
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertTrue

/**
 * The assistant, driven headlessly: ask for something and watch the answer
 * assemble itself out of a stream.
 */
@OptIn(ExperimentalTestApi::class)
class AssistantUiTest {
    private fun ComposeUiTest.openAssistant() {
        waitUntilExactlyOneExists(hasTestTag(TAG_NEEDS_YOU))
        onNodeWithTag(tabTag(SignedTab.Assistant)).performClick()
        waitUntilExactlyOneExists(hasTestTag(TAG_ASSISTANT))
    }

    private fun ComposeUiTest.ask(text: String) {
        onNodeWithTag(TAG_ASSISTANT_COMPOSER).performTextInput(text)
        onNodeWithTag(TAG_ASSISTANT_SEND).performClick()
    }

    /** Content deltas concatenate into one answer, not one message per token. */
    @Test
    fun a_streamed_reply_assembles_into_one_answer() =
        runComposeUiTest {
            setContent { App(FakePorts().ports(), testStore(signedIn = true)) }
            openAssistant()

            ask("why did the deploy stall?")

            waitUntilExactlyOneExists(hasTextExactly("The deploy stalled on a failing migration."))
            // Two turns: the question and the reply.
            onAllNodesWithTag(TAG_ASSISTANT_TURN).assertCountEquals(2)
            onNodeWithText("why did the deploy stall?").assertExists()
        }

    @Test
    fun the_turn_carries_the_accounts_preferred_model() =
        runComposeUiTest {
            val ports = FakePorts()
            setContent { App(ports.ports(), testStore(signedIn = true)) }
            openAssistant()

            ask("hello")
            waitUntil { ports.chatTurns.isNotEmpty() }
            assertEquals("claude-opus-5", ports.chatTurns.single().first)
            assertEquals("hello", ports.chatTurns.single().second)
            // First turn of a thread: no conversation to continue.
            assertEquals(null, ports.chatTurns.single().third)
        }

    /**
     * The conversation id arrives in a header before any token, and keeping it
     * is what makes a second question continue the same thread instead of
     * starting a new one.
     */
    @Test
    fun a_second_question_continues_the_same_thread() =
        runComposeUiTest {
            val ports = FakePorts()
            setContent { App(ports.ports(), testStore(signedIn = true)) }
            openAssistant()

            ask("first")
            waitUntilExactlyOneExists(hasTextExactly("The deploy stalled on a failing migration."))
            ask("second")
            waitUntil { ports.chatTurns.size == 2 }

            assertEquals(null, ports.chatTurns[0].third)
            assertEquals("conv-1", ports.chatTurns[1].third, "the second turn must carry the conversation id")
        }

    /** A fresh thread is explicit: agent threads are per-topic here, and
     *  continuing one forever is how context rot sets in. */
    @Test
    fun a_new_thread_forgets_the_conversation() =
        runComposeUiTest {
            val ports = FakePorts()
            setContent { App(ports.ports(), testStore(signedIn = true)) }
            openAssistant()

            ask("first")
            waitUntilExactlyOneExists(hasTestTag(TAG_ASSISTANT_NEW))
            onNodeWithTag(TAG_ASSISTANT_NEW).performClick()
            onAllNodesWithTag(TAG_ASSISTANT_TURN).assertCountEquals(0)

            ask("fresh")
            waitUntil { ports.chatTurns.size == 2 }
            assertEquals(null, ports.chatTurns[1].third, "a new thread starts with no conversation id")
        }

    /**
     * Tool progress is shown, because an agent running tools is the main reason
     * a reply takes thirty seconds — hide it and a working assistant looks
     * frozen.
     */
    @Test
    fun tool_progress_is_visible_and_resolves_to_done() =
        runComposeUiTest {
            val ports =
                FakePorts().apply {
                    chatScript =
                        listOf(
                            ChatEvent.Started("conv-1", "msg-1"),
                            ChatEvent.Tool(id = "t1", name = "bash", label = "🔧 running tests", status = ToolStatus.Running),
                            // The completion frame carries NO label — the row
                            // must keep the name it was running under.
                            ChatEvent.Tool(id = "t1", name = "bash", label = "", status = ToolStatus.Completed),
                            ChatEvent.Content("All green."),
                        )
                }
            setContent { App(ports.ports(), testStore(signedIn = true)) }
            openAssistant()

            ask("run the tests")
            waitUntilExactlyOneExists(hasTextExactly("All green."))

            onAllNodesWithTag(TAG_ASSISTANT_TOOL).assertCountEquals(1)
            onNodeWithText("🔧 running tests").assertExists()
            onNodeWithText("done").assertExists()
        }

    /** Reasoning is kept apart from the answer. */
    @Test
    fun reasoning_is_shown_separately_from_the_answer() =
        runComposeUiTest {
            val ports =
                FakePorts().apply {
                    chatScript =
                        listOf(
                            ChatEvent.Started("conv-1", null),
                            ChatEvent.Reasoning("checking the migration log"),
                            ChatEvent.Content("It was the migration."),
                        )
                }
            setContent { App(ports.ports(), testStore(signedIn = true)) }
            openAssistant()

            ask("why?")
            waitUntilExactlyOneExists(hasTextExactly("It was the migration."))
            onNodeWithTag(TAG_ASSISTANT_REASONING).assertTextEquals("checking the migration log")
        }

    @Test
    fun usage_is_shown_when_the_stream_reports_it() =
        runComposeUiTest {
            val ports =
                FakePorts().apply {
                    chatScript =
                        listOf(
                            ChatEvent.Started("conv-1", null),
                            ChatEvent.Content("done"),
                            ChatEvent.Usage(promptTokens = 120, completionTokens = 45),
                        )
                }
            setContent { App(ports.ports(), testStore(signedIn = true)) }
            openAssistant()

            ask("go")
            waitUntilExactlyOneExists(hasTestTag(TAG_ASSISTANT_USAGE))
            onNodeWithTag(TAG_ASSISTANT_USAGE).assertTextEquals("120 in · 45 out")
        }

    /** A 202 means the reply joined history rather than streaming. Saying so
     *  beats a turn that just sits there empty. */
    @Test
    fun a_queued_reply_says_so() =
        runComposeUiTest {
            val ports =
                FakePorts().apply {
                    chatScript = listOf(ChatEvent.Started("conv-1", null), ChatEvent.Queued)
                }
            setContent { App(ports.ports(), testStore(signedIn = true)) }
            openAssistant()

            ask("go")
            waitUntilExactlyOneExists(hasTestTag(TAG_ASSISTANT_QUEUED))
        }

    @Test
    fun a_failed_stream_says_why() =
        runComposeUiTest {
            val ports =
                FakePorts().apply {
                    chatScript = listOf(ChatEvent.Failed("Outcrop Labs answered 503"))
                }
            setContent { App(ports.ports(), testStore(signedIn = true)) }
            openAssistant()

            ask("go")
            waitUntilExactlyOneExists(hasTestTag(TAG_ASSISTANT_ERROR))
            onNodeWithTag(TAG_ASSISTANT_ERROR).assertTextContains("503", substring = true)
        }

    @Test
    fun an_expired_session_mid_stream_signs_the_account_out() =
        runComposeUiTest {
            val ports = FakePorts().apply { chatScript = listOf(ChatEvent.Expired) }
            val store = testStore(signedIn = true)
            setContent { App(ports.ports(), store) }
            openAssistant()

            ask("go")
            waitUntilExactlyOneExists(hasTestTag(Tags.INSTANCE_URL))
            assertTrue(store.accounts().isEmpty())
        }

    /**
     * No model, no guessing. Choosing one is configuration — a desk surface by
     * the scope line — and a guess would quietly spend somebody's money on a
     * tier they never picked.
     */
    @Test
    fun an_account_with_no_preferred_model_is_told_rather_than_guessed_for() =
        runComposeUiTest {
            val ports = FakePorts()
            setContent { App(ports.ports(), testStore(signedIn = true, preferredModel = null)) }
            openAssistant()

            onNodeWithTag(TAG_ASSISTANT_NO_MODEL).assertExists()
            onNodeWithTag(TAG_ASSISTANT_COMPOSER).assertDoesNotExist()
            assertTrue(ports.chatTurns.isEmpty(), "nothing may be sent without a model")
        }

    @Test
    fun the_send_button_is_dead_on_an_empty_box() =
        runComposeUiTest {
            setContent { App(FakePorts().ports(), testStore(signedIn = true)) }
            openAssistant()
            onNodeWithTag(TAG_ASSISTANT_SEND).assertIsNotEnabled()
            onNodeWithTag(TAG_ASSISTANT_COMPOSER).performTextInput("   ")
            onNodeWithTag(TAG_ASSISTANT_SEND).assertIsNotEnabled()
        }

    /** The box clears on send: re-reading your own question in the composer
     *  while the answer streams in reads as "it did not send". */
    @Test
    fun the_composer_clears_on_send() =
        runComposeUiTest {
            setContent { App(FakePorts().ports(), testStore(signedIn = true)) }
            openAssistant()

            ask("why did the deploy stall?")
            waitUntilExactlyOneExists(hasTextExactly("The deploy stalled on a failing migration."))
            onNodeWithTag(TAG_ASSISTANT_COMPOSER).assertTextContains("")
        }
}
