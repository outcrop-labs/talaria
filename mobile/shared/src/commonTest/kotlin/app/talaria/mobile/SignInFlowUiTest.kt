package app.talaria.mobile

import androidx.compose.ui.test.*
import app.talaria.mobile.account.Account
import app.talaria.mobile.account.Credential
import app.talaria.mobile.account.InMemoryAccountStore
import app.talaria.mobile.account.Me
import app.talaria.mobile.account.MeResult
import app.talaria.mobile.net.HomeResult
import app.talaria.mobile.screens.TAG_BLOCKED
import app.talaria.mobile.screens.TAG_NEEDS_YOU
import app.talaria.mobile.screens.TAG_NOTICE
import app.talaria.mobile.screens.TAG_REVIEW
import app.talaria.mobile.screens.TAG_TRAILER
import app.talaria.mobile.screens.TAG_TRIAGE
import app.talaria.mobile.screens.TAG_WORK_ITEM
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertTrue

/**
 * Phase 1 and the landing surface, driven headlessly: prove an instance, sign
 * in, and read what is waiting on you.
 */
@OptIn(ExperimentalTestApi::class)
class SignInFlowUiTest {
    /** The full arc, from an empty app to a decision queue. */
    @Test
    fun add_an_instance_sign_in_and_see_what_is_waiting() =
        runComposeUiTest {
            setContent { App(FakePorts().ports(), InMemoryAccountStore()) }

            onNodeWithTag(Tags.INSTANCE_URL).performTextInput("talaria.example.com")
            onNodeWithTag(Tags.ADD_INSTANCE).performClick()
            waitUntilExactlyOneExists(hasTestTag(Tags.INSTANCE_ROW))

            onNodeWithTag(Tags.SIGN_IN).performClick() // launcher → sign-in
            onNodeWithTag(Tags.USERNAME).performTextInput("jon@outcroplabs.com")
            onNodeWithTag(Tags.PASSWORD).performTextInput(FakePorts.CORRECT_PASSWORD)
            onNodeWithTag(Tags.SIGN_IN).performClick() // submit

            waitUntilExactlyOneExists(hasTestTag(TAG_NEEDS_YOU))

            // /api/me supplied a display name, so the username is not what shows.
            onNodeWithTag(Tags.ACCOUNT_NAME).assertTextEquals("Jon Iler")
            // review 2 + blocked 1 + triage 0; unread is excluded.
            onNodeWithTag(TAG_NEEDS_YOU).assertTextEquals("3 waiting on you")
            onNodeWithTag(TAG_REVIEW).assertExists()
            onNodeWithTag(TAG_BLOCKED).assertExists()
            onNodeWithTag(TAG_TRAILER).assertTextContains("4 unread", substring = true)

            // A ticket with a ref reads as "TASK-12 · title"; one without is the
            // bare title. Same rule as the api's own WorkItem::label.
            onNodeWithText("TASK-12 · Fix the login loop").assertExists()
            onNodeWithText("Rename the gateway flag").assertExists()
        }

    /** When /api/me has no display name, the username stands in rather than a
     *  blank where a person's name should be. */
    @Test
    fun an_account_with_no_profile_name_shows_its_username() =
        runComposeUiTest {
            val ports = FakePorts().apply { onMe = { _, _ -> MeResult.Ok(Me(name = null)) } }
            setContent { App(ports.ports(), testStore(signedIn = false)) }

            onNodeWithTag(Tags.SIGN_IN).performClick()
            onNodeWithTag(Tags.USERNAME).performTextInput("sam@outcroplabs.com")
            onNodeWithTag(Tags.PASSWORD).performTextInput(FakePorts.CORRECT_PASSWORD)
            onNodeWithTag(Tags.SIGN_IN).performClick()

            waitUntilExactlyOneExists(hasTestTag(TAG_NEEDS_YOU))
            onNodeWithTag(Tags.ACCOUNT_NAME).assertTextEquals("sam@outcroplabs.com")
        }

    /** An empty queue is absent, not a heading with nothing under it. */
    @Test
    fun a_queue_with_nothing_in_it_is_absent_not_blank() =
        runComposeUiTest {
            setContent { App(FakePorts().ports(), testStore(signedIn = true)) }
            waitUntilExactlyOneExists(hasTestTag(TAG_NEEDS_YOU))
            onNodeWithTag(TAG_TRIAGE).assertDoesNotExist()
        }

    @Test
    fun nothing_waiting_says_so_rather_than_showing_zeroes() =
        runComposeUiTest {
            val ports =
                FakePorts().apply {
                    onHome = { _, _ -> HomeResult.Ok(sampleHome(review = 0, blocked = 0, triage = 0, unread = 0)) }
                }
            setContent { App(ports.ports(), testStore(signedIn = true)) }

            waitUntilExactlyOneExists(hasTestTag(TAG_NEEDS_YOU))
            onNodeWithTag(TAG_NEEDS_YOU).assertTextEquals("Nothing is waiting on you.")
            onAllNodesWithTag(TAG_WORK_ITEM).assertCountEquals(0)
        }

    /** The count is the queue; the items are a window onto it. A list that
     *  silently stops short is worse than one that says it did. */
    @Test
    fun a_queue_larger_than_its_window_says_how_many_more() =
        runComposeUiTest {
            val ports = FakePorts().apply { onHome = { _, _ -> HomeResult.Ok(sampleHome(review = 9)) } }
            setContent { App(ports.ports(), testStore(signedIn = true)) }

            waitUntilExactlyOneExists(hasTestTag(TAG_NEEDS_YOU))
            onNodeWithText("+7 more").assertExists() // count 9, two items listed
        }

    @Test
    fun a_wrong_password_keeps_you_on_the_form_with_a_reason() =
        runComposeUiTest {
            setContent { App(FakePorts().ports(), testStore(signedIn = false)) }

            onNodeWithTag(Tags.SIGN_IN).performClick()
            onNodeWithTag(Tags.USERNAME).performTextInput("jon@outcroplabs.com")
            onNodeWithTag(Tags.PASSWORD).performTextInput("wrong")
            onNodeWithTag(Tags.SIGN_IN).performClick()

            waitUntilExactlyOneExists(hasTestTag(Tags.SIGN_IN_ERROR))
            onNodeWithTag(Tags.SIGN_IN_ERROR).assertTextContains("did not match", substring = true)
            onNodeWithTag(TAG_NEEDS_YOU).assertDoesNotExist()
        }

    @Test
    fun a_home_that_cannot_be_reached_says_why_and_keeps_the_account() =
        runComposeUiTest {
            val ports =
                FakePorts().apply {
                    onHome = { _, _ -> HomeResult.Failed("could not reach Outcrop Labs: no answer") }
                }
            val store = testStore(signedIn = true)
            setContent { App(ports.ports(), store) }

            waitUntilExactlyOneExists(hasTestTag(Tags.ADD_ERROR))
            onNodeWithTag(Tags.ADD_ERROR).assertTextContains("could not reach", substring = true)
            // Unreachable is not unauthorized: a tunnel that dropped must not
            // cost someone their session.
            assertEquals(1, store.accounts().size)
        }

    /**
     * An expired session is the 7-day absolute TTL doing what it says. The
     * account is dropped — leaving it would mean every screen 401s — and the
     * launcher says why rather than silently forgetting someone. This whole
     * behaviour is what gap 1 exists to remove.
     */
    @Test
    fun an_expired_session_signs_the_account_out_and_says_so() =
        runComposeUiTest {
            val ports = FakePorts().apply { onHome = { _, _ -> HomeResult.Expired } }
            val store = testStore(signedIn = true)
            setContent { App(ports.ports(), store) }

            waitUntilExactlyOneExists(hasTestTag(TAG_NOTICE))
            onNodeWithTag(TAG_NOTICE).assertTextContains("session expired", substring = true)
            assertTrue(store.accounts().isEmpty(), "an expired credential must not be kept")
            onNodeWithTag(Tags.INSTANCE_URL).assertExists() // back on the launcher
        }

    @Test
    fun signing_out_returns_to_the_launcher_and_forgets_the_credential() =
        runComposeUiTest {
            val store = testStore(signedIn = true)
            setContent { App(FakePorts().ports(), store) }

            waitUntilExactlyOneExists(hasTestTag(TAG_NEEDS_YOU))
            onNodeWithTag(Tags.SIGN_OUT).performClick()

            waitUntilExactlyOneExists(hasTestTag(Tags.INSTANCE_URL))
            assertTrue(store.accounts().isEmpty())
            assertEquals(1, store.instances().size, "signing out of an account keeps the instance")
        }

    /** Two people on ONE instance — the case desktop structurally cannot serve,
     *  because its isolation is one cookie jar per webview. */
    @Test
    fun one_instance_can_hold_two_signed_in_accounts() =
        runComposeUiTest {
            setContent { App(FakePorts().ports(), twoAccountStore()) }

            onAllNodesWithTag(Tags.ACCOUNT_ROW).assertCountEquals(2)
            onNodeWithText("Jon").assertExists()
            onNodeWithText("Sam").assertExists()
            // Two accounts is a genuine question, so the launcher asks it
            // instead of guessing which one you meant.
            onNodeWithTag(TAG_NEEDS_YOU).assertDoesNotExist()
        }

    /** Each account's own credential rides its own request. There is no ambient
     *  cookie jar to mix them up — the isolation desktop gets from the OS, a
     *  phone has to arrange for itself. */
    @Test
    fun opening_an_account_uses_that_accounts_credential() =
        runComposeUiTest {
            val ports = FakePorts()
            setContent { App(ports.ports(), twoAccountStore()) }

            onNodeWithText("Sam").performClick()
            waitUntilExactlyOneExists(hasTestTag(TAG_NEEDS_YOU))

            assertEquals<List<Credential>>(listOf(Credential.Session("sid-sam")), ports.homeCredentials)
        }

    /** And switching to the other one sends the other one's credential — not a
     *  cached first answer. */
    @Test
    fun switching_accounts_switches_credentials() =
        runComposeUiTest {
            val ports = FakePorts()
            setContent { App(ports.ports(), twoAccountStore()) }

            onNodeWithText("Sam").performClick()
            waitUntilExactlyOneExists(hasTestTag(TAG_NEEDS_YOU))
            onNodeWithTag(Tags.SWITCH_ACCOUNT).performClick()

            waitUntilExactlyOneExists(hasTestTag(Tags.INSTANCE_URL))
            onNodeWithText("Jon").performClick()
            waitUntil { ports.homeCredentials.size == 2 }

            assertEquals<List<Credential>>(
                listOf(Credential.Session("sid-sam"), Credential.Session("sid-jon")),
                ports.homeCredentials,
            )
        }

    @Test
    fun removing_an_instance_takes_its_accounts_with_it() =
        runComposeUiTest {
            val store = testStore(signedIn = true)
            setContent { App(FakePorts().ports(), store) }

            waitUntilExactlyOneExists(hasTestTag(TAG_NEEDS_YOU))
            onNodeWithTag(Tags.SWITCH_ACCOUNT).performClick()
            waitUntilExactlyOneExists(hasTestTag(Tags.REMOVE_INSTANCE))
            onNodeWithTag(Tags.REMOVE_INSTANCE).performClick()

            onNodeWithTag(Tags.EMPTY).assertExists()
            assertTrue(store.instances().isEmpty())
            assertTrue(store.accounts().isEmpty(), "a credential for a forgotten instance can never be revoked")
        }

    private fun twoAccountStore() =
        InMemoryAccountStore(
            instances = listOf(testInstance()),
            signedIn =
                mapOf(
                    Account(TEST_UUID, "jon@outcroplabs.com", "Jon") to Credential.Session("sid-jon"),
                    Account(TEST_UUID, "sam@outcroplabs.com", "Sam") to Credential.Session("sid-sam"),
                ),
        )
}
