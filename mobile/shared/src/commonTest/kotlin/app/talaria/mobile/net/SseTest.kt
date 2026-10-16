package app.talaria.mobile.net

import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertNull
import kotlin.test.assertTrue

/**
 * The SSE parser, string in and frames out.
 *
 * Worth the length: this is the layer that decides whether a live surface
 * updates or quietly stops, and every failure mode here is invisible from the
 * UI. The two that would actually have shipped are
 * [a_ping_is_not_an_event][pings_and_the_preamble_are_not_events] — Talaria
 * sends a comment every 25 seconds, so a parser that read comments as data
 * would invent an event every 25 seconds forever — and
 * [a_frame_split_across_chunks_still_arrives], because a network chunk
 * boundary does not respect frame boundaries and the bug only appears under
 * load.
 */
class SseTest {
    private fun frames(vararg chunks: String): List<SseFrame> {
        val parser = SseParser()
        return chunks.flatMap { parser.feed(it) }
    }

    @Test
    fun one_frame_in_one_chunk() {
        val got = frames("data: hello\n\n")
        assertEquals(1, got.size)
        assertEquals("hello", got[0].data)
        assertNull(got[0].event)
    }

    /** Nothing is emitted until the blank line. */
    @Test
    fun a_frame_is_not_emitted_until_its_blank_line() {
        val parser = SseParser()
        assertTrue(parser.feed("data: hello\n").isEmpty())
        assertEquals(1, parser.feed("\n").size)
    }

    /** The case that breaks under load and never in a demo. */
    @Test
    fun a_frame_split_across_chunks_still_arrives() {
        val got = frames("da", "ta: hel", "lo\n", "\n")
        assertEquals(listOf("hello"), got.map { it.data })
    }

    /** Talaria opens with `: connected` and pings every 25 seconds. If these
     *  became events, every live screen would twitch on a timer. */
    @Test
    fun pings_and_the_preamble_are_not_events() {
        val got = frames(": connected\n\n", ": ping\n\n", "data: real\n\n", ": ping\n\n")
        assertEquals(listOf("real"), got.map { it.data })
    }

    @Test
    fun several_data_lines_join_with_newlines() {
        val got = frames("data: one\ndata: two\ndata: three\n\n")
        assertEquals("one\ntwo\nthree", got.single().data)
    }

    @Test
    fun the_event_name_is_carried_and_does_not_leak_to_the_next_frame() {
        val got = frames("event: moved\ndata: a\n\n", "data: b\n\n")
        assertEquals("moved", got[0].event)
        assertNull(got[1].event, "an event name must not survive its own frame")
    }

    @Test
    fun an_id_is_carried_and_does_not_leak() {
        val got = frames("id: 7\ndata: a\n\n", "data: b\n\n")
        assertEquals("7", got[0].id)
        assertNull(got[1].id)
    }

    /** One space after the colon is framing; the rest is data. A payload that
     *  is deliberately indented must survive. */
    @Test
    fun exactly_one_leading_space_is_stripped() {
        assertEquals("hello", frames("data: hello\n\n").single().data)
        assertEquals("hello", frames("data:hello\n\n").single().data)
        assertEquals(" hello", frames("data:  hello\n\n").single().data)
    }

    @Test
    fun crlf_servers_are_understood() {
        val got = frames("data: hello\r\n\r\n")
        assertEquals(listOf("hello"), got.map { it.data })
    }

    /** A lone trailing `\r` might be the first half of a `\r\n` whose `\n` is
     *  in the next chunk. Dispatching on it would split one frame into two. */
    @Test
    fun a_cr_at_a_chunk_boundary_waits_for_its_lf() {
        val parser = SseParser()
        assertTrue(parser.feed("data: hello\r").isEmpty())
        assertTrue(parser.feed("\n").isEmpty(), "that was the line end, not the frame end")
        assertEquals(1, parser.feed("\r\n").size)
    }

    /**
     * Bare `\r` terminators work — but only once something follows them, and
     * that asymmetry is correct rather than a shortcoming. A lone `\r` at the
     * end of the buffer is indistinguishable from the first half of a `\r\n`
     * whose `\n` is in the next packet, so the parser has to wait. Guessing
     * would split one frame into two under exactly the conditions nobody
     * reproduces locally.
     */
    @Test
    fun bare_cr_line_endings_are_understood_once_something_follows() {
        assertEquals(listOf("hello"), frames("data: hello\r\rdata: next\r").map { it.data })
    }

    @Test
    fun a_trailing_lone_cr_waits_rather_than_guessing() {
        val parser = SseParser()
        assertTrue(parser.feed("data: hello\r\r").isEmpty(), "the second CR might be a split CRLF")
        // Whichever it turns out to be, the frame lands intact and once.
        assertEquals(listOf("hello"), parser.feed("\n").map { it.data })
    }

    @Test
    fun a_frame_with_no_data_is_not_dispatched() {
        assertTrue(frames("event: moved\n\n").isEmpty())
        assertTrue(frames("\n\n\n").isEmpty())
    }

    /** `data:` with nothing after it is an empty-string event, which is a real
     *  event — distinct from a frame with no data line at all. */
    @Test
    fun an_empty_data_line_is_an_event_with_empty_data() {
        val got = frames("data:\n\n")
        assertEquals(1, got.size)
        assertEquals("", got.single().data)
    }

    @Test
    fun unknown_fields_are_ignored() {
        val got = frames("retry: 5000\nsomething: else\ndata: a\n\n")
        assertEquals("a", got.single().data)
    }

    @Test
    fun several_frames_in_one_chunk_all_arrive() {
        val got = frames("data: a\n\ndata: b\n\ndata: c\n\n")
        assertEquals(listOf("a", "b", "c"), got.map { it.data })
    }

    @Test
    fun a_realistic_talaria_stream_reads_correctly() {
        val got =
            frames(
                ": connected\n\n",
                """data: {"type":"notification","notificationId":"n1"}""" + "\n\n",
                ": ping\n\n",
                """data: {"type":"channel","channelId":"c1"}""" + "\n\n",
            )
        assertEquals(2, got.size)
        assertEquals(UserEvent.Notification("n1"), decodeUserEvent(got[0].data))
        assertEquals(UserEvent.Channel("c1"), decodeUserEvent(got[1].data))
    }
}

/**
 * The `UserEvent` wire contract — `tag = "type"`, lowercase variant names,
 * camelCase fields, ids only.
 */
class UserEventDecodeTest {
    @Test
    fun every_variant_decodes_from_the_apis_spelling() {
        assertEquals(
            UserEvent.Run(runId = "r1", state = "awaiting"),
            decodeUserEvent("""{"type":"run","runId":"r1","state":"awaiting"}"""),
        )
        assertEquals(
            UserEvent.Notification("n1"),
            decodeUserEvent("""{"type":"notification","notificationId":"n1"}"""),
        )
        assertEquals(
            UserEvent.Brief(briefId = "b1", seq = 12),
            decodeUserEvent("""{"type":"brief","briefId":"b1","seq":12}"""),
        )
        assertEquals(UserEvent.Channel("c1"), decodeUserEvent("""{"type":"channel","channelId":"c1"}"""))
        assertEquals(
            UserEvent.Conversation("v1"),
            decodeUserEvent("""{"type":"conversation","conversationId":"v1"}"""),
        )
        assertEquals(
            UserEvent.Typing(channelId = "c1", userId = "u1", typing = true),
            decodeUserEvent("""{"type":"typing","channelId":"c1","userId":"u1","typing":true}"""),
        )
    }

    /** An api newer than the app must not kill the stream. */
    @Test
    fun an_unknown_event_type_is_dropped_rather_than_thrown() {
        assertNull(decodeUserEvent("""{"type":"somethingNew","id":"x"}"""))
        assertNull(decodeUserEvent("not json at all"))
        assertNull(decodeUserEvent(""))
    }

    /** A field added to an event this build DOES know is tolerated. */
    @Test
    fun an_unknown_field_on_a_known_event_is_tolerated() {
        assertEquals(
            UserEvent.Channel("c1"),
            decodeUserEvent("""{"type":"channel","channelId":"c1","somethingNew":1}"""),
        )
    }

    /**
     * The invariant, asserted rather than trusted: no variant carries content.
     * If a future event grew a `text` or `title`, realtime would start widening
     * reads — and this test is where that gets caught.
     */
    @Test
    fun no_event_carries_content() {
        val payload = """{"type":"channel","channelId":"c1","text":"the secret message"}"""
        val event = decodeUserEvent(payload)
        assertEquals(UserEvent.Channel("c1"), event)
        // The id is everything the client learns; the body is unreachable from
        // the typed event, so the client has no choice but to re-fetch through
        // the ordinary ACL'd route.
        assertTrue(
            event.toString().contains("c1") && !event.toString().contains("secret"),
            "an event must carry ids only: $event",
        )
    }
}
