package app.talaria.mobile.net

/**
 * An incremental Server-Sent Events parser.
 *
 * Talaria's realtime plane is Redis pub/sub → SSE, with no websockets anywhere
 * (`docs/ARCHITECTURE.md`), so this is the shape every live surface in the app
 * arrives through: the decision queue updating itself, a message landing, an
 * agent's reply tokenizing.
 *
 * IT IS A SEPARATE, PURE OBJECT ON PURPOSE. Frame parsing is the part that is
 * easy to get subtly wrong and impossible to debug from a UI — a chunk boundary
 * that lands mid-event, a `\r\n` server, a comment line mistaken for data — and
 * all of it is testable with strings and no socket at all. Everything that
 * needs a connection lives in [userEvents]; everything that needs thought
 * lives here.
 *
 * The rules it implements, from the SSE specification and from what Talaria's
 * own stream actually sends:
 *  - Lines end with `\n`, `\r\n` or a bare `\r`.
 *  - A BLANK line dispatches the frame accumulated so far. Nothing is emitted
 *    until then, which is exactly why this has to be incremental: a `data:`
 *    line can be split across two network chunks.
 *  - A line starting with `:` is a comment and is dropped. Talaria relies on
 *    this twice — a `: connected` preamble, and a `: ping` every 25 seconds to
 *    hold the connection open — so a parser that treated comments as data
 *    would surface a phantom event every 25 seconds, forever.
 *  - Several `data:` lines in one frame join with `\n`.
 *  - One optional space after the colon is stripped; further spaces are data.
 *  - A frame carrying no `data` at all is not dispatched.
 */
data class SseFrame(
    val event: String? = null,
    val data: String,
    val id: String? = null,
)

class SseParser {
    private val pending = StringBuilder()
    private val data = StringBuilder()
    private var event: String? = null
    private var id: String? = null
    private var sawData = false

    /** Feed one network chunk; get back every frame it completed. */
    fun feed(chunk: String): List<SseFrame> {
        val out = mutableListOf<SseFrame>()
        pending.append(chunk)

        while (true) {
            val line = takeLine() ?: break
            if (line.isEmpty()) {
                dispatch()?.let { out += it }
                continue
            }
            if (line.startsWith(":")) continue // comment: the preamble and the pings

            val colon = line.indexOf(':')
            val field = if (colon < 0) line else line.substring(0, colon)
            // One optional leading space is part of the framing, not the value.
            // Any further space IS the value, which matters for a payload that
            // is deliberately indented.
            val value =
                when {
                    colon < 0 -> ""
                    line.length > colon + 1 && line[colon + 1] == ' ' -> line.substring(colon + 2)
                    else -> line.substring(colon + 1)
                }

            when (field) {
                "data" -> {
                    if (sawData) data.append('\n')
                    data.append(value)
                    sawData = true
                }
                "event" -> event = value.takeIf { it.isNotEmpty() }
                "id" -> id = value.takeIf { it.isNotEmpty() }
                // `retry` and anything unknown are ignored: reconnection
                // backoff is the caller's policy, not the parser's.
                else -> Unit
            }
        }
        return out
    }

    /**
     * Pull one complete line out of the buffer, or null when the buffer holds
     * no terminator yet.
     *
     * The `\r` case needs care: a lone trailing `\r` might be the first half of
     * a `\r\n` whose `\n` is in the next chunk. Treating it as a line end now
     * would dispatch early and then see a stray empty line, so it waits.
     */
    private fun takeLine(): String? {
        var i = 0
        while (i < pending.length) {
            val c = pending[i]
            if (c == '\n') {
                val line = pending.substring(0, i)
                pending.deleteRange(0, i + 1)
                return line
            }
            if (c == '\r') {
                if (i == pending.length - 1) return null // might be CRLF, split
                val skip = if (pending[i + 1] == '\n') 2 else 1
                val line = pending.substring(0, i)
                pending.deleteRange(0, i + skip)
                return line
            }
            i++
        }
        return null
    }

    private fun dispatch(): SseFrame? {
        if (!sawData) {
            // A frame with no data is not an event. Reset anyway: an `event:`
            // line with nothing after it must not leak into the next frame.
            event = null
            id = null
            return null
        }
        val frame = SseFrame(event = event, data = data.toString(), id = id)
        data.clear()
        event = null
        id = null
        sawData = false
        return frame
    }
}
