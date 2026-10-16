package app.talaria.mobile.net

import kotlinx.serialization.json.Json
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertNull
import kotlin.test.assertTrue

/**
 * The watch shapes: the live work session, and the token ledger.
 *
 * The session is hand-built JSON in the route (`tasks_id_work_session.rs`)
 * rather than a serialized struct, so the key names are literally written out
 * there — `runId`, `agentModel`, `lastTail` — with no `rename_all` to infer
 * from. The ledger is `CostOverview` from `talaria-gateway`, where the OUTER
 * structs carry camelCase and `CostWindow` / `CostPerDay` deliberately do not,
 * because every field in them is one word.
 */
class WatchParseTest {
    private val json = Json { ignoreUnknownKeys = true; explicitNulls = false }

    @Test
    fun a_live_session_decodes() {
        val payload = """
            {"session":{"runId":"run-1","state":"running","phase":"acting",
             "agentModel":"Muse","turn":7,"lastTail":"Re-ran the migration."},
             "wait":3}
            """
        val read = json.decodeFromString<WorkSessionRead>(payload)
        val session = read.session!!
        assertEquals("run-1", session.runId)
        assertEquals("acting", session.phase)
        assertEquals("Muse", session.agentModel)
        assertEquals(7, session.turn)
        assertEquals("Re-ran the migration.", session.lastTail)
        assertEquals(3, read.wait)
    }

    /** Nothing running is an explicit null session plus a wait hint — a real
     *  answer, which is why the route bothers to send it. */
    @Test
    fun no_session_is_a_real_answer() {
        val read = json.decodeFromString<WorkSessionRead>("""{"session":null,"wait":5}""")
        assertNull(read.session)
        assertEquals(5, read.wait)
    }

    /** The route reads turn and lastTail out of a checkpoint with `and_then`,
     *  so either can legitimately be absent mid-run. */
    @Test
    fun a_session_without_a_checkpoint_yet_still_decodes() {
        val read =
            json.decodeFromString<WorkSessionRead>(
                """{"session":{"runId":"run-1","state":"queued","phase":null,
                   "agentModel":null,"turn":null,"lastTail":null},"wait":1}""",
            )
        val session = read.session!!
        assertEquals("run-1", session.runId)
        assertNull(session.turn)
        assertNull(session.lastTail)
    }

    private val ledgerPayload = """
        {
          "totals":{
            "today":{"prompt":90000,"completion":30000,"cache":5000,"generations":214,"cost":4.19},
            "week":{"prompt":500000,"completion":150000,"cache":20000,"generations":1400,"cost":31.5},
            "month":{"prompt":2000000,"completion":600000,"cache":90000,"generations":5600,"cost":128},
            "estimatedShare":0.4,
            "split":{"local":100,"cloud":900,"other":0},
            "unpricedCloudTokens":0
          },
          "perModel":[{"llmModel":"claude-opus-5","endpointClass":"cloud","tokens":120000,"cost":4.19}],
          "perAgent":[
            {"agentModel":"Muse","prompt":60000,"completion":20000,"generations":120,
             "lastUsed":"2026-10-14T09:00:00Z","cost":2.8,"localShare":0},
            {"agentModel":null,"prompt":1000,"completion":200,"generations":4,
             "lastUsed":null,"cost":0.05,"localShare":null}
          ],
          "perDay":[{"day":"2026-10-14","prompt":90000,"completion":30000,"generations":214,
                     "local":0,"cloud":214}]
        }
        """

    @Test
    fun the_ledger_decodes_and_drops_what_is_not_modelled() {
        val overview = json.decodeFromString<CostOverview>(ledgerPayload)
        assertEquals(4.19, overview.totals.today.cost)
        assertEquals(214, overview.totals.today.generations)
        assertEquals(2, overview.perAgent.size)
        // estimatedShare, split, unpricedCloudTokens, perModel and perDay were
        // all on the wire and none are modelled.
    }

    /** `tokens` is prompt + completion and deliberately excludes cache: a cache
     *  read is not work the model did. */
    @Test
    fun tokens_is_prompt_plus_completion_and_excludes_cache() {
        val today = json.decodeFromString<CostOverview>(ledgerPayload).totals.today
        assertEquals(120_000, today.tokens)
        assertEquals(5_000, today.cache, "cache is carried, just not counted as work")
    }

    /**
     * `js_numberify` makes a whole dollar print as `128` rather than `128.0`.
     * Both have to decode to the same Double, or a month's spend reads as a
     * parse error.
     */
    @Test
    fun a_whole_dollar_arrives_as_an_integer_and_still_decodes() {
        val overview = json.decodeFromString<CostOverview>(ledgerPayload)
        assertEquals(128.0, overview.totals.month.cost)
        assertEquals(31.5, overview.totals.week.cost)
    }

    /** Spend with no agent attributed to it is real and must still render. */
    @Test
    fun unattributed_spend_is_labelled_rather_than_blank() {
        val agents = json.decodeFromString<CostOverview>(ledgerPayload).perAgent
        assertEquals("Muse", agents[0].shown)
        assertNull(agents[1].agentModel)
        assertEquals("unattributed", agents[1].shown)
    }

    /** A fresh instance has spent nothing, and that decodes to zeroes rather
     *  than to absences. */
    @Test
    fun an_empty_ledger_decodes_to_zeroes() {
        val overview = json.decodeFromString<CostOverview>("""{}""")
        assertEquals(0.0, overview.totals.today.cost)
        assertEquals(0, overview.totals.today.tokens)
        assertTrue(overview.perAgent.isEmpty())
    }
}
