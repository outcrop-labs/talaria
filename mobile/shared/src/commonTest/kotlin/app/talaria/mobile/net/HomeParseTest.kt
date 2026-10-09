package app.talaria.mobile.net

import kotlinx.serialization.json.Json
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertNull
import kotlin.test.assertTrue

/**
 * The `/api/home` wire contract.
 *
 * The UI tests fake the port, so this is where the SHAPE is held to account —
 * and it matters more here than for most routes, because the generated
 * reference prints `…` for this one (the response is computed, not a literal),
 * so the only sources of truth are the api's structs. The JSON below is written
 * to match them field for field: `api/crates/talaria-home/src/lib.rs`'s
 * `HomeSummary`, `HomeQueues`, `QueueBucket`, `WorkItem`, `OrgGlance`.
 *
 * The three things that would silently break a screen:
 *   - `WorkItem` and `OrgGlance` carry `rename_all = "camelCase"`, so the wire
 *     says `boardId`, `ticketRef`, `updatedAt`, `costToday`. Get one wrong and
 *     the field decodes as null rather than failing.
 *   - `alerts` and `costToday` are null for a member and present for an admin.
 *   - `org.activity` is on the wire and is deliberately NOT modelled, so the
 *     decoder has to tolerate it.
 */
class HomeParseTest {
    private val json = Json { ignoreUnknownKeys = true; explicitNulls = false }

    private val memberPayload = """
        {
          "org": {
            "name": "Outcrop Labs",
            "activity": [{"id":"a1","kind":"ticket.moved","at":"2026-10-08T09:00:00Z"}],
            "alerts": null,
            "costToday": null
          },
          "queues": {
            "triage": {"count": 0, "items": []},
            "review": {"count": 3, "items": [
              {"id":"t1","boardId":"b1","board":"Platform","ticketRef":"TASK-12",
               "title":"Fix the login loop","status":"quality_review",
               "updatedAt":"2026-10-08T09:00:00Z","queue":"review"},
              {"id":"t2","boardId":"b1","board":"Platform","ticketRef":null,
               "title":"Rename the gateway flag","status":"quality_review",
               "updatedAt":"2026-10-08T08:00:00Z","queue":"review"}
            ]},
            "blocked": {"count": 1, "items": [
              {"id":"t3","boardId":"b2","board":"Growth","ticketRef":"GRO-4",
               "title":"Needs a decision on pricing","status":"blocked",
               "updatedAt":"2026-10-07T17:00:00Z","queue":"blocked"}
            ]}
          },
          "unread": 4,
          "boards": 3
        }
        """

    @Test
    fun a_members_home_decodes_whole() {
        val home = json.decodeFromString<HomeSummary>(memberPayload)

        assertEquals("Outcrop Labs", home.org.name)
        assertEquals(4, home.unread)
        assertEquals(3, home.boards)

        // camelCase fields actually landed — a wrong name decodes as null, so
        // asserting the VALUE is the only way to catch it.
        val first = home.queues.review.items.first()
        assertEquals("b1", first.boardId)
        assertEquals("TASK-12", first.ticketRef)
        assertEquals("2026-10-08T09:00:00Z", first.updatedAt)
        assertEquals("quality_review", first.status)
    }

    /** An admin's extra fields, which a member never sees. */
    @Test
    fun an_admins_alerts_and_cost_decode() {
        val admin =
            memberPayload.replace(
                """"alerts": null,
            "costToday": null""",
                """"alerts": 2,
            "costToday": {"tokens": 148231, "usd": 4.21}""",
            )
        val home = json.decodeFromString<HomeSummary>(admin)
        assertEquals(2, home.org.alerts)
        assertEquals(148231, home.org.costToday?.tokens)
        assertEquals(4.21, home.org.costToday?.usd)
    }

    @Test
    fun a_members_nulls_stay_null_rather_than_zero() {
        val home = json.decodeFromString<HomeSummary>(memberPayload)
        // The difference matters: 0 alerts is a fact about the instance, null
        // is "you are not allowed to know", and a member must not be shown an
        // authoritative zero.
        assertNull(home.org.alerts)
        assertNull(home.org.costToday)
    }

    /** The count is the queue; items are a window. The screen renders
     *  "+N more" off this difference, so the decoder must not conflate them. */
    @Test
    fun the_count_is_not_the_item_count() {
        val home = json.decodeFromString<HomeSummary>(memberPayload)
        assertEquals(3, home.queues.review.count)
        assertEquals(2, home.queues.review.items.size)
    }

    @Test
    fun needs_you_sums_the_three_queues_and_excludes_unread() {
        val home = json.decodeFromString<HomeSummary>(memberPayload)
        // review 3 + blocked 1 + triage 0. Unread is 4 and must not be in it:
        // a message is not a decision.
        assertEquals(4, home.needsYou)
        assertTrue(home.unread == 4)
    }

    /** `WorkItem::label` in the api: `TASK-12 · title`, or the bare title. */
    @Test
    fun the_label_matches_the_apis_own_rule() {
        val home = json.decodeFromString<HomeSummary>(memberPayload)
        assertEquals("TASK-12 · Fix the login loop", home.queues.review.items[0].label)
        assertEquals("Rename the gateway flag", home.queues.review.items[1].label)
    }

    /** A field the api adds tomorrow must not break an app shipped today. */
    @Test
    fun unknown_fields_are_tolerated() {
        val widened = memberPayload.replace(""""unread": 4""", """"somethingNew": {"a":1}, "unread": 4""")
        assertEquals(4, json.decodeFromString<HomeSummary>(widened).unread)
    }

    /** An empty instance answers with zeroes, not absences. */
    @Test
    fun a_quiet_instance_decodes_to_nothing_waiting() {
        val quiet = """{"org":{"name":"Outcrop Labs","activity":[]},"queues":{},"unread":0,"boards":0}"""
        val home = json.decodeFromString<HomeSummary>(quiet)
        assertEquals(0, home.needsYou)
        assertTrue(home.queues.review.items.isEmpty())
    }
}
