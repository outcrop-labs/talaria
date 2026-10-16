package app.talaria.mobile.text

import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.SpanStyle
import androidx.compose.ui.text.buildAnnotatedString
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontStyle
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.withStyle

/**
 * Turning the api's text into something Compose can draw.
 *
 * Two converters, both pure, both here because they are the part that is easy
 * to get quietly wrong — and in the first case, wrong in a way that would have
 * shipped.
 */

/**
 * A KB search snippet.
 *
 * **The snippet is HTML, not text, and that is the catch.** `ts_headline` does
 * not escape the document it excerpts, so the api escapes the whole snippet
 * itself and then swaps its own control-character sentinels for `<b>` — the
 * whole XSS boundary lives in `talaria-kb`'s `safe_snippet`, deliberately, so
 * that every consumer is safe rather than just the one that renders today. The
 * web client then renders it with `{@html}`.
 *
 * A Compose client that drew that string as text would show a reader literal
 * `<b>` tags and `&amp;` entities. So this undoes exactly what the api did:
 * `<b>…</b>` becomes a bold span, and the five escapes it produces become their
 * characters again. Nothing else in the string is treated as markup, because
 * nothing else in it is — the api escaped the rest.
 */
fun snippetToAnnotated(snippet: String): AnnotatedString =
    buildAnnotatedString {
        var i = 0
        var bold = false
        val buffer = StringBuilder()

        fun flush() {
            if (buffer.isEmpty()) return
            val text = buffer.toString()
            if (bold) {
                withStyle(SpanStyle(fontWeight = FontWeight.Bold)) { append(text) }
            } else {
                append(text)
            }
            buffer.clear()
        }

        while (i < snippet.length) {
            when {
                snippet.startsWith("<b>", i) -> {
                    flush()
                    bold = true
                    i += 3
                }
                snippet.startsWith("</b>", i) -> {
                    flush()
                    bold = false
                    i += 4
                }
                snippet[i] == '&' -> {
                    val semi = snippet.indexOf(';', i)
                    // A bare ampersand is a character, not a broken entity.
                    val entity = if (semi > i && semi - i <= 6) snippet.substring(i, semi + 1) else null
                    val decoded = entity?.let { ENTITIES[it] }
                    if (decoded != null) {
                        buffer.append(decoded)
                        i = semi + 1
                    } else {
                        buffer.append('&')
                        i++
                    }
                }
                else -> {
                    buffer.append(snippet[i])
                    i++
                }
            }
        }
        flush()
    }

/** Exactly what the api's escaper produces, and nothing more. */
private val ENTITIES =
    mapOf(
        "&amp;" to "&",
        "&lt;" to "<",
        "&gt;" to ">",
        "&quot;" to "\"",
        "&#39;" to "'",
        "&apos;" to "'",
    )

/**
 * A markdown subset, for READING a knowledge doc or a plan.
 *
 * WHAT THIS IS NOT: a markdown engine. Knowledge and Artifacts are a versioned
 * markdown drive with tables, images, embeds and nesting, and rendering all of
 * that properly is a project — the scope doc named it as the single largest UI
 * cost in the whole app, which is why reading is last and editing is never.
 *
 * What it handles is what a decision needs when the queue did not carry enough
 * context: headings, bullets, numbered items, quotes, and inline `code`,
 * **bold** and *italic*. Everything else — tables, images, links' targets,
 * fenced-block languages — degrades to its own source text rather than
 * disappearing, because a reader who can see the raw line can still act on it,
 * while a reader shown nothing cannot.
 *
 * Fenced blocks are passed through verbatim on purpose: a code block is the one
 * place where inline markers are usually literal, and "helpfully" bolding a
 * `**` inside a shell snippet is how a rendered doc starts lying about its
 * contents.
 */
data class MarkdownBlock(
    val text: AnnotatedString,
    val kind: BlockKind,
    /** Bullet/number depth, 0 for a top-level item. */
    val indent: Int = 0,
)

enum class BlockKind { Heading1, Heading2, Heading3, Paragraph, Bullet, Numbered, Quote, Code, Blank }

fun parseMarkdown(body: String): List<MarkdownBlock> {
    val out = mutableListOf<MarkdownBlock>()
    val lines = body.replace("\r\n", "\n").replace('\r', '\n').split('\n')
    var inFence = false

    for (line in lines) {
        val fence = line.trimStart().startsWith("```")
        if (fence) {
            // The fence markers themselves are not content.
            inFence = !inFence
            continue
        }
        if (inFence) {
            out += MarkdownBlock(AnnotatedString(line), BlockKind.Code)
            continue
        }

        val trimmed = line.trimStart()
        val indent = (line.length - trimmed.length) / 2

        when {
            trimmed.isEmpty() -> out += MarkdownBlock(AnnotatedString(""), BlockKind.Blank)
            trimmed.startsWith("### ") ->
                out += MarkdownBlock(inline(trimmed.removePrefix("### ")), BlockKind.Heading3)
            trimmed.startsWith("## ") ->
                out += MarkdownBlock(inline(trimmed.removePrefix("## ")), BlockKind.Heading2)
            trimmed.startsWith("# ") ->
                out += MarkdownBlock(inline(trimmed.removePrefix("# ")), BlockKind.Heading1)
            trimmed.startsWith("> ") ->
                out += MarkdownBlock(inline(trimmed.removePrefix("> ")), BlockKind.Quote, indent)
            trimmed.startsWith("- ") || trimmed.startsWith("* ") ->
                out += MarkdownBlock(inline(trimmed.drop(2)), BlockKind.Bullet, indent)
            NUMBERED.matches(trimmed) ->
                out += MarkdownBlock(
                    inline(trimmed.substringAfter(". ")),
                    BlockKind.Numbered,
                    indent,
                )
            else -> out += MarkdownBlock(inline(trimmed), BlockKind.Paragraph, indent)
        }
    }
    // Trailing blanks are noise at the end of a document.
    return out.dropLastWhile { it.kind == BlockKind.Blank }
}

private val NUMBERED = Regex("^\\d+\\. .*")

/**
 * Inline markers within one line: `code`, **bold**, *italic*.
 *
 * Deliberately single-pass and non-nesting. Nested emphasis is rare in the
 * documents this reads and a half-correct nesting implementation produces
 * worse output than none — an unmatched marker stays as the character it is,
 * which is what a reader expects from a plain-text fallback.
 */
internal fun inline(line: String): AnnotatedString =
    buildAnnotatedString {
        var i = 0
        while (i < line.length) {
            val rest = line.substring(i)
            val code = span(rest, "`")
            val bold = span(rest, "**")
            val italic = if (!rest.startsWith("**")) span(rest, "*") else null

            when {
                code != null -> {
                    withStyle(SpanStyle(fontFamily = FontFamily.Monospace)) { append(code) }
                    i += code.length + 2
                }
                bold != null -> {
                    withStyle(SpanStyle(fontWeight = FontWeight.Bold)) { append(bold) }
                    i += bold.length + 4
                }
                italic != null -> {
                    withStyle(SpanStyle(fontStyle = FontStyle.Italic)) { append(italic) }
                    i += italic.length + 2
                }
                else -> {
                    append(line[i])
                    i++
                }
            }
        }
    }

/** The content between a marker and its next match, or null when this is not
 *  the start of a closed span. An unclosed marker is just a character. */
private fun span(
    rest: String,
    marker: String,
): String? {
    if (!rest.startsWith(marker)) return null
    val close = rest.indexOf(marker, marker.length)
    if (close <= marker.length) return null // unclosed, or empty: `` is not code
    return rest.substring(marker.length, close)
}
