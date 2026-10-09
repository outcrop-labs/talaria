package app.talaria.mobile

// A wildcard here on purpose: a behaviour test leans on a dozen matchers and
// assertions from this one package, and listing them adds noise without adding
// information.
import androidx.compose.ui.test.*
import app.talaria.mobile.account.InMemoryAccountStore
import app.talaria.mobile.instance.Beacon
import app.talaria.mobile.instance.ProbeResult
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertTrue

@OptIn(ExperimentalTestApi::class)
class AddInstanceUiTest {
    @Test
    fun a_real_instance_is_added_and_labelled_by_its_company_name() =
        runComposeUiTest {
            setContent { App(FakePorts().ports(), InMemoryAccountStore()) }

            onNodeWithTag(Tags.EMPTY).assertExists()
            onNodeWithTag(Tags.INSTANCE_URL).performTextInput("talaria.example.com")
            onNodeWithTag(Tags.ADD_INSTANCE).performClick()

            waitUntilExactlyOneExists(hasTestTag(Tags.INSTANCE_ROW))
            onNodeWithText("Outcrop Labs").assertExists()
            onNodeWithText("https://talaria.example.com").assertExists()
            onNodeWithTag(Tags.EMPTY).assertDoesNotExist()
        }

    /** The probe is asked about the NORMALIZED origin, not about whatever the
     *  person typed — host case folded, path and query gone. */
    @Test
    fun the_probe_goes_to_the_normalized_origin() =
        runComposeUiTest {
            val ports = FakePorts()
            setContent { App(ports.ports(), InMemoryAccountStore()) }

            onNodeWithTag(Tags.INSTANCE_URL).performTextInput("TALARIA.example.com/some/path?x=1")
            onNodeWithTag(Tags.ADD_INSTANCE).performClick()
            waitUntilExactlyOneExists(hasTestTag(Tags.INSTANCE_ROW))

            assertEquals(listOf("https://talaria.example.com"), ports.probed)
        }

    @Test
    fun a_url_that_cannot_be_an_origin_never_reaches_the_network() =
        runComposeUiTest {
            val ports = FakePorts()
            setContent { App(ports.ports(), InMemoryAccountStore()) }

            onNodeWithTag(Tags.INSTANCE_URL).performTextInput("ftp://example.com")
            onNodeWithTag(Tags.ADD_INSTANCE).performClick()

            waitUntilExactlyOneExists(hasTestTag(Tags.ADD_ERROR))
            assertTrue(ports.probed.isEmpty(), "a refused URL must not be dialled")
            onNodeWithTag(Tags.EMPTY).assertExists()
        }

    @Test
    fun something_that_is_not_talaria_is_refused_with_its_reason() =
        runComposeUiTest {
            val ports =
                FakePorts().apply {
                    onProbe = { origin ->
                        ProbeResult.Failed(
                            "$origin answered 404 at /api/well-known/talaria-instance — " +
                                "it does not look like a Talaria instance",
                        )
                    }
                }
            setContent { App(ports.ports(), InMemoryAccountStore()) }

            onNodeWithTag(Tags.INSTANCE_URL).performTextInput("nginx.example.com")
            onNodeWithTag(Tags.ADD_INSTANCE).performClick()

            waitUntilExactlyOneExists(hasTestTag(Tags.ADD_ERROR))
            onNodeWithTag(Tags.ADD_ERROR).assertTextContains("404", substring = true)
            onNodeWithTag(Tags.EMPTY).assertExists()
        }

    /** The beacon uuid is the dedupe key, so one instance reached by two names
     *  is one row — the rule the desktop registry holds. */
    @Test
    fun the_same_instance_under_two_names_is_one_row() =
        runComposeUiTest {
            val ports = FakePorts()
            setContent { App(ports.ports(), InMemoryAccountStore()) }

            onNodeWithTag(Tags.INSTANCE_URL).performTextInput("talaria.example.com")
            onNodeWithTag(Tags.ADD_INSTANCE).performClick()
            waitUntilExactlyOneExists(hasTestTag(Tags.INSTANCE_ROW))

            onNodeWithTag(Tags.INSTANCE_URL).performTextInput("talaria.example.com:443")
            onNodeWithTag(Tags.ADD_INSTANCE).performClick()
            waitUntil { ports.probed.size == 2 }

            onAllNodesWithTag(Tags.INSTANCE_ROW).assertCountEquals(1)
        }

    /** Two genuinely different instances are two rows. */
    @Test
    fun two_different_instances_are_two_rows() =
        runComposeUiTest {
            val ports =
                FakePorts().apply {
                    onProbe = { origin ->
                        // A distinct uuid per origin, as two real instances have.
                        val id = if (origin.contains("other")) "11111111-2222-3333-4444-555555555555" else TEST_UUID
                        ProbeResult.Found(origin, Beacon(instance = id, companyName = origin))
                    }
                }
            setContent { App(ports.ports(), InMemoryAccountStore()) }

            onNodeWithTag(Tags.INSTANCE_URL).performTextInput("talaria.example.com")
            onNodeWithTag(Tags.ADD_INSTANCE).performClick()
            waitUntilExactlyOneExists(hasTestTag(Tags.INSTANCE_ROW))

            onNodeWithTag(Tags.INSTANCE_URL).performTextInput("other.example.com")
            onNodeWithTag(Tags.ADD_INSTANCE).performClick()
            waitUntilAtLeastOneExists(hasTestTag(Tags.INSTANCE_ROW))
            waitUntil { ports.probed.size == 2 }

            onAllNodesWithTag(Tags.INSTANCE_ROW).assertCountEquals(2)
        }

    @Test
    fun the_button_is_dead_until_something_is_typed() =
        runComposeUiTest {
            setContent { App(FakePorts().ports(), InMemoryAccountStore()) }
            onNodeWithTag(Tags.ADD_INSTANCE).assertIsNotEnabled()
        }

    /** Typing again clears the previous complaint: a stale error beside a field
     *  someone is already fixing reads as a new one. */
    @Test
    fun editing_the_field_clears_the_last_error() =
        runComposeUiTest {
            setContent { App(FakePorts().ports(), InMemoryAccountStore()) }

            onNodeWithTag(Tags.INSTANCE_URL).performTextInput("ftp://example.com")
            onNodeWithTag(Tags.ADD_INSTANCE).performClick()
            waitUntilExactlyOneExists(hasTestTag(Tags.ADD_ERROR))

            onNodeWithTag(Tags.INSTANCE_URL).performTextInput("x")
            onNodeWithTag(Tags.ADD_ERROR).assertDoesNotExist()
        }
}
