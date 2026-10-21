package app.talaria.mobile.net

import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertNull
import kotlin.test.assertTrue

/**
 * The chat stream's frame vocabulary.
 *
 * These are deliberately the same cases `ui/src/lib/sse-parse.ts` handles,
 * because both clients read the same stream and a disagreement means one of
 * them renders a reply wrong. The port is the reason this file exists: the
 * scope doc claimed that module was genuinely portable, and this is the claim
 * being cashed.
 */
class ChatParseTest {
    private fun one(
        data: String,
        event: String? = null,
    ) = parseChatFrame(event, data)

    /** `[DONE]` is the terminator and is NOT json. Parsing it would throw, and
     *  a thrown error here would end a reply that had just completed. */
    @Test
    fun the_done_sentinel_is_not_an_event() {
        assertTrue(one("[DONE]").isEmpty())
        assertTrue(one("").isEmpty())
    }

    @Test
    fun a_content_delta_becomes_content() {
        val events = one("""{"choices":[{"delta":{"content":"Hello"}}]}""")
        assertEquals(listOf(ChatEvent.Content("Hello")), events)
    }

    /** Reasoning is shown apart from the answer, so it must not arrive as
     *  content — and the api spells it two ways. */
    @Test
    fun reasoning_arrives_under_either_spelling() {
        assertEquals(
            listOf(ChatEvent.Reasoning("thinking")),
            one("""{"choices":[{"delta":{"reasoning":"thinking"}}]}"""),
        )
        assertEquals(
            listOf(ChatEvent.Reasoning("thinking")),
            one("""{"choices":[{"delta":{"reasoning_content":"thinking"}}]}"""),
        )
    }

    /** Content wins when a chunk somehow carries both: the answer is what the
     *  person asked for. */
    @Test
    fun content_takes_precedence_over_reasoning() {
        assertEquals(
            listOf(ChatEvent.Content("answer")),
            one("""{"choices":[{"delta":{"content":"answer","reasoning":"aside"}}]}"""),
        )
    }

    /** One chunk can carry a delta AND the final usage. Returning a single
     *  event would drop the only cost signal the stream gives. */
    @Test
    fun a_chunk_can_carry_both_a_delta_and_usage() {
        val events =
            one(
                """{"choices":[{"delta":{"content":"end"}}],
                   "usage":{"prompt_tokens":120,"completion_tokens":45}}""",
            )
        assertEquals(
            listOf(ChatEvent.Content("end"), ChatEvent.Usage(120, 45)),
            events,
        )
    }

    /** A usage block of zeroes is the absence of a measurement, not a
     *  measurement of zero, and must not render as a cost. */
    @Test
    fun an_all_zero_usage_block_is_not_an_event() {
        assertTrue(one("""{"usage":{"prompt_tokens":0,"completion_tokens":0}}""").isEmpty())
    }

    @Test
    fun an_empty_delta_says_nothing() {
        assertTrue(one("""{"choices":[{"delta":{}}]}""").isEmpty())
        assertTrue(one("""{"choices":[]}""").isEmpty())
        assertTrue(one("""{"choices":[{"delta":{"content":""}}]}""").isEmpty())
    }

    /** Keep-alives and half-frames ride this stream by design. */
    @Test
    fun unparseable_frames_are_dropped_rather_than_thrown() {
        assertTrue(one("not json").isEmpty())
        assertTrue(one("{").isEmpty())
        assertTrue(one(": keep-alive").isEmpty())
    }

    // ---- tool progress ----------------------------------------------------

    /** Matched on the EVENT NAME before any json parse — the payload is a
     *  different shape entirely, and parsing it as a chunk finds nothing. */
    @Test
    fun a_tool_progress_frame_is_matched_by_its_event_name() {
        val payload = """{"tool":"bash","emoji":"🔧","label":"running tests","toolCallId":"tc1","status":"running"}"""
        val events = one(payload, event = "hermes.tool.progress")
        assertEquals(
            listOf(ChatEvent.Tool(id = "tc1", name = "bash", label = "🔧 running tests", status = ToolStatus.Running)),
            events,
        )
        // The same payload with no event name is read as a chunk, and a chunk
        // it is not.
        assertTrue(one(payload).isEmpty())
    }

    @Test
    fun both_harnesses_progress_events_are_understood() {
        val payload = """{"name":"grep","label":"searching","tool_call_id":"tc2","status":"completed"}"""
        listOf("hermes.tool.progress", "claude.tool.progress").forEach { name ->
            val tool = one(payload, event = name).single() as ChatEvent.Tool
            assertEquals("tc2", tool.id, "snake_case tool_call_id must be read too")
            assertEquals("grep", tool.name)
            assertEquals(ToolStatus.Completed, tool.status)
        }
    }

    @Test
    fun an_unnamed_tool_falls_back_rather_than_going_blank() {
        val tool = one("""{"label":"doing something","toolCallId":"tc3"}""", event = "hermes.tool.progress")
            .single() as ChatEvent.Tool
        assertEquals("tool", tool.name)
        assertNull(tool.status)
    }

    /** Nothing to show and nothing to merge on. */
    @Test
    fun a_tool_frame_with_no_label_and_no_id_is_dropped() {
        assertTrue(one("""{"tool":"bash","status":"running"}""", event = "hermes.tool.progress").isEmpty())
    }

    /** `complete` and `completed` both appear in the wild. */
    @Test
    fun either_spelling_of_completed_is_understood() {
        listOf("complete", "completed", "COMPLETED").forEach { spelling ->
            val tool = one(
                """{"tool":"bash","label":"x","toolCallId":"t","status":"$spelling"}""",
                event = "hermes.tool.progress",
            ).single() as ChatEvent.Tool
            assertEquals(ToolStatus.Completed, tool.status, "status '$spelling'")
        }
    }
}

/**
 * Folding tool events into a list.
 *
 * The empty-label rule is the one that bites: a "completed" frame carries no
 * label, so a careless merge blanks the name of the tool that just finished.
 */
class MergeToolTest {
    private fun running(
        id: String?,
        name: String,
        label: String,
    ) = ChatEvent.Tool(id = id, name = name, label = label, status = ToolStatus.Running)

    @Test
    fun a_new_tool_is_appended_and_defaults_to_running() {
        val tools = mergeTool(emptyList(), ChatEvent.Tool(id = "a", name = "bash", label = "x"))
        assertEquals(1, tools.size)
        assertEquals(ToolStatus.Running, tools.single().status, "no status means it has started")
    }

    @Test
    fun the_same_id_updates_in_place() {
        val first = mergeTool(emptyList(), running("a", "bash", "running tests"))
        val done = mergeTool(first, ChatEvent.Tool(id = "a", name = "bash", label = "", status = ToolStatus.Completed))
        assertEquals(1, done.size)
        assertEquals(ToolStatus.Completed, done.single().status)
    }

    /** THE one. A completion frame has no label; the finished tool must keep
     *  the name it was running under. */
    @Test
    fun a_completion_frame_does_not_blank_the_label() {
        val first = mergeTool(emptyList(), running("a", "bash", "🔧 running tests"))
        val done = mergeTool(first, ChatEvent.Tool(id = "a", name = "bash", label = "", status = ToolStatus.Completed))
        assertEquals("🔧 running tests", done.single().label)
    }

    /** With no id, a running tool of the same name is the one being updated. */
    @Test
    fun without_an_id_a_running_tool_of_the_same_name_is_matched() {
        val first = mergeTool(emptyList(), running(null, "bash", "first"))
        val done = mergeTool(first, ChatEvent.Tool(name = "bash", label = "", status = ToolStatus.Completed))
        assertEquals(1, done.size)
        assertEquals(ToolStatus.Completed, done.single().status)
    }

    /** ...and once it has completed, the same name starting again is a NEW
     *  tool, not a resurrection of the finished one. */
    @Test
    fun a_completed_tool_is_not_matched_by_a_later_run_of_the_same_name() {
        val first = mergeTool(emptyList(), running(null, "bash", "first"))
        val done = mergeTool(first, ChatEvent.Tool(name = "bash", label = "", status = ToolStatus.Completed))
        val again = mergeTool(done, running(null, "bash", "second"))
        assertEquals(2, again.size)
        assertEquals(listOf("first", "second"), again.map { it.label })
    }

    @Test
    fun different_ids_are_different_tools() {
        var tools = mergeTool(emptyList(), running("a", "bash", "one"))
        tools = mergeTool(tools, running("b", "bash", "two"))
        assertEquals(2, tools.size)
    }
}
