package app.talaria.mobile

// A wildcard here on purpose: a behaviour test leans on a dozen matchers and
// assertions from this one package, and listing them adds noise without adding
// information.
import androidx.compose.ui.test.*
import app.talaria.mobile.instance.BEACON_PATH
import io.ktor.http.HttpStatusCode
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertTrue

@OptIn(ExperimentalTestApi::class)
class AddInstanceUiTest {
    @Test
    fun a_real_instance_is_added_and_labelled_by_its_company_name() =
        runComposeUiTest {
            val api = FakeApi().json(BEACON_PATH, beaconJson(companyName = "Outcrop Labs"))
            setContent { App(api.client()) }

            onNodeWithTag(Tags.EMPTY).assertExists()
            onNodeWithTag(Tags.INSTANCE_URL).performTextInput("talaria.example.com")
            onNodeWithTag(Tags.ADD_INSTANCE).performClick()

            waitUntilExactlyOneExists(hasTextExactly("Outcrop Labs"))
            onNodeWithText("https://talaria.example.com").assertExists()
            onNodeWithTag(Tags.EMPTY).assertDoesNotExist()
        }

    /** The beacon is asked at its documented path, on the normalized origin —
     *  not on whatever the person typed. */
    @Test
    fun the_probe_goes_to_the_normalized_origin() =
        runComposeUiTest {
            val api = FakeApi().json(BEACON_PATH, beaconJson())
            setContent { App(api.client()) }

            onNodeWithTag(Tags.INSTANCE_URL).performTextInput("TALARIA.example.com/some/path?x=1")
            onNodeWithTag(Tags.ADD_INSTANCE).performClick()
            waitUntilExactlyOneExists(hasTextExactly("Outcrop Labs"))

            assertEquals(1, api.seen.size)
            val asked = api.seen.single().url
            assertEquals("talaria.example.com", asked.host)
            assertEquals(BEACON_PATH, asked.encodedPath)
            assertEquals("https", asked.protocol.name)
        }

    @Test
    fun a_url_that_cannot_be_an_origin_never_reaches_the_network() =
        runComposeUiTest {
            val api = FakeApi() // no routes at all: any request would error loudly
            setContent { App(api.client()) }

            onNodeWithTag(Tags.INSTANCE_URL).performTextInput("ftp://example.com")
            onNodeWithTag(Tags.ADD_INSTANCE).performClick()

            waitUntilExactlyOneExists(hasTestTag(Tags.ADD_ERROR))
            assertTrue(api.seen.isEmpty(), "a refused URL must not be dialled")
            onNodeWithTag(Tags.EMPTY).assertExists()
        }

    @Test
    fun something_that_is_not_talaria_is_refused_with_its_status() =
        runComposeUiTest {
            val api = FakeApi().status(BEACON_PATH, HttpStatusCode.NotFound)
            setContent { App(api.client()) }

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
            val api = FakeApi().json(BEACON_PATH, beaconJson(companyName = "Outcrop Labs"))
            setContent { App(api.client()) }

            onNodeWithTag(Tags.INSTANCE_URL).performTextInput("talaria.example.com")
            onNodeWithTag(Tags.ADD_INSTANCE).performClick()
            waitUntilExactlyOneExists(hasTextExactly("Outcrop Labs"))

            onNodeWithTag(Tags.INSTANCE_URL).performTextInput("talaria.example.com:443")
            onNodeWithTag(Tags.ADD_INSTANCE).performClick()
            waitUntil { api.seen.size == 2 }

            onAllNodesWithTag(Tags.INSTANCE_ROW).assertCountEquals(1)
        }

    @Test
    fun the_button_is_dead_until_something_is_typed() =
        runComposeUiTest {
            setContent { App(FakeApi().client()) }
            onNodeWithTag(Tags.ADD_INSTANCE).assertIsNotEnabled()
        }

    /** Typing again clears the previous complaint: a stale error beside a field
     *  someone is already fixing reads as a new one. */
    @Test
    fun editing_the_field_clears_the_last_error() =
        runComposeUiTest {
            setContent { App(FakeApi().client()) }

            onNodeWithTag(Tags.INSTANCE_URL).performTextInput("ftp://example.com")
            onNodeWithTag(Tags.ADD_INSTANCE).performClick()
            waitUntilExactlyOneExists(hasTestTag(Tags.ADD_ERROR))

            onNodeWithTag(Tags.INSTANCE_URL).performTextInput("x")
            onNodeWithTag(Tags.ADD_ERROR).assertDoesNotExist()
        }
}
