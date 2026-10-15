package app.talaria.mobile.text

import androidx.compose.ui.text.font.FontWeight
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertTrue

/**
 * The search snippet converter.
 *
 * This exists because of a real catch: `KbSearchHit.snippet` is **HTML, not
 * text**. `ts_headline` does not escape the document it excerpts, so the api
 * escapes the whole snippet itself and swaps its own control-character
 * sentinels for `<b>` — the XSS boundary lives server-side in `safe_snippet`
 * so that every consumer is safe. The web client renders it with `{@html}`. A
 * Compose client that drew the same string as text would show a reader literal
 * `<b>` tags and `&amp;` entities, which is exactly the bug these cases exist
 * to prevent.
 */
class SnippetTest {
    private fun plain(snippet: String) = snippetToAnnotated(snippet).text

    private fun boldRanges(snippet: String) =
        snippetToAnnotated(snippet)
            .spanStyles
            .filter { it.item.fontWeight == FontWeight.Bold }
            .map { snippetToAnnotated(snippet).text.substring(it.start, it.end) }

    @Test
    fun plain_text_passes_through() {
        assertEquals("the deploy stalled", plain("the deploy stalled"))
        assertTrue(boldRanges("the deploy stalled").isEmpty())
    }

    @Test
    fun bold_tags_become_a_bold_span_and_leave_no_markup_behind() {
        val snippet = "the <b>deploy</b> stalled"
        assertEquals("the deploy stalled", plain(snippet))
        assertEquals(listOf("deploy"), boldRanges(snippet))
    }

    @Test
    fun several_highlights_each_get_their_own_span() {
        val snippet = "<b>deploy</b> and <b>migration</b>"
        assertEquals("deploy and migration", plain(snippet))
        assertEquals(listOf("deploy", "migration"), boldRanges(snippet))
    }

    /** The five escapes the api's escaper produces, undone. */
    @Test
    fun the_apis_entities_are_decoded() {
        assertEquals("a & b", plain("a &amp; b"))
        assertEquals("<img>", plain("&lt;img&gt;"))
        assertEquals("\"quoted\"", plain("&quot;quoted&quot;"))
        assertEquals("it's", plain("it&#39;s"))
    }

    /**
     * The whole point of the api escaping first. A doc body containing a script
     * tag arrives as entities, and must render as the VISIBLE TEXT of that tag
     * — never as markup this converter re-interprets.
     */
    @Test
    fun an_escaped_tag_renders_as_text_not_as_markup() {
        val snippet = "look: &lt;img src=x onerror=alert(1)&gt;"
        assertEquals("look: <img src=x onerror=alert(1)>", plain(snippet))
        assertTrue(boldRanges(snippet).isEmpty())
    }

    /** A real `<b>` in a document would have been escaped to `&lt;b&gt;` by the
     *  api, so an unescaped one is the api's own highlight — and nothing else
     *  in the string is treated as a tag. */
    @Test
    fun other_tags_are_left_alone_as_characters() {
        // Not a highlight marker, so it is not markup to this converter.
        assertEquals("<i>kept</i>", plain("<i>kept</i>"))
    }

    @Test
    fun a_bare_ampersand_is_a_character_not_a_broken_entity() {
        assertEquals("Tom & Jerry", plain("Tom & Jerry"))
        assertEquals("a &notanentity; b", plain("a &notanentity; b"))
    }

    @Test
    fun an_empty_snippet_is_empty() {
        assertEquals("", plain(""))
    }

    /** An unclosed highlight should still show its text rather than swallow the
     *  rest of the snippet. */
    @Test
    fun an_unclosed_bold_tag_still_renders_its_text() {
        assertEquals("the deploy", plain("the <b>deploy"))
        assertEquals(listOf("deploy"), boldRanges("the <b>deploy"))
    }
}

/**
 * The markdown subset.
 *
 * NOT a markdown engine — Knowledge is a versioned markdown drive and rendering
 * all of it is the biggest UI cost in the app's scope. What matters here is
 * that the subset handles what a decision needs, and that everything it does
 * not know degrades to its own source line rather than vanishing: a reader who
 * can see the raw line can still act on it.
 */
class MarkdownTest {
    private fun kinds(body: String) = parseMarkdown(body).map { it.kind }

    private fun texts(body: String) = parseMarkdown(body).map { it.text.text }

    @Test
    fun headings_are_recognised_at_three_levels() {
        assertEquals(
            listOf(BlockKind.Heading1, BlockKind.Heading2, BlockKind.Heading3),
            kinds("# One\n## Two\n### Three"),
        )
        assertEquals(listOf("One", "Two", "Three"), texts("# One\n## Two\n### Three"))
    }

    /** A hash with no space is not a heading — it is a tag, or a comment. */
    @Test
    fun a_hash_without_a_space_is_not_a_heading() {
        assertEquals(listOf(BlockKind.Paragraph), kinds("#hashtag"))
    }

    @Test
    fun bullets_and_numbers_are_recognised_with_either_marker() {
        assertEquals(
            listOf(BlockKind.Bullet, BlockKind.Bullet, BlockKind.Numbered),
            kinds("- one\n* two\n1. three"),
        )
        assertEquals(listOf("one", "two", "three"), texts("- one\n* two\n1. three"))
    }

    @Test
    fun indentation_becomes_a_depth() {
        val blocks = parseMarkdown("- top\n  - nested\n    - deeper")
        assertEquals(listOf(0, 1, 2), blocks.map { it.indent })
    }

    @Test
    fun quotes_are_recognised() {
        assertEquals(listOf(BlockKind.Quote), kinds("> a quotation"))
        assertEquals(listOf("a quotation"), texts("> a quotation"))
    }

    /**
     * A fenced block is passed through verbatim, and the fences themselves are
     * not content. "Helpfully" bolding a `**` inside a shell snippet is how a
     * rendered doc starts lying about what it contains.
     */
    @Test
    fun a_fenced_block_is_verbatim_and_its_markers_are_not_content() {
        val body = "before\n```sh\nrm -rf **/*.tmp\n```\nafter"
        val blocks = parseMarkdown(body)
        assertEquals(
            listOf(BlockKind.Paragraph, BlockKind.Code, BlockKind.Paragraph),
            blocks.map { it.kind },
        )
        assertEquals("rm -rf **/*.tmp", blocks[1].text.text, "inline markers inside a fence are literal")
    }

    @Test
    fun inline_code_bold_and_italic_are_styled_and_their_markers_removed() {
        assertEquals("a code b", texts("a `code` b").single())
        assertEquals("a bold b", texts("a **bold** b").single())
        assertEquals("a italic b", texts("a *italic* b").single())
    }

    /** Bold is checked before italic, or `**x**` renders as italic-star-x-star. */
    @Test
    fun bold_wins_over_italic_on_a_double_marker() {
        val block = parseMarkdown("**bold**").single()
        assertEquals("bold", block.text.text)
        assertTrue(
            block.text.spanStyles.any { it.item.fontWeight == FontWeight.Bold },
            "a double marker is bold, not italic with stray stars",
        )
    }

    /** An unclosed or empty marker is just a character — which is what a reader
     *  expects from a plain-text fallback, and better than swallowing the line. */
    @Test
    fun unclosed_and_empty_markers_stay_as_characters() {
        assertEquals("a *dangling", texts("a *dangling").single())
        assertEquals("100% ** done", texts("100% ** done").single())
        assertEquals("an `unclosed", texts("an `unclosed").single())
    }

    /** Things the subset does not know keep their own source text. */
    @Test
    fun unsupported_syntax_degrades_to_its_source_line() {
        val table = "| a | b |\n|---|---|\n| 1 | 2 |"
        assertEquals(listOf(BlockKind.Paragraph, BlockKind.Paragraph, BlockKind.Paragraph), kinds(table))
        assertEquals("| a | b |", texts(table).first())

        // A link keeps its text AND its target, which is more useful on a phone
        // than a tappable link this screen could not open anyway.
        assertEquals("see [the doc](https://x.example)", texts("see [the doc](https://x.example)").single())
    }

    @Test
    fun blank_lines_are_blocks_but_trailing_ones_are_dropped() {
        assertEquals(listOf(BlockKind.Paragraph, BlockKind.Blank, BlockKind.Paragraph), kinds("a\n\nb"))
        assertEquals(listOf(BlockKind.Paragraph), kinds("a\n\n\n"))
    }

    @Test
    fun crlf_documents_parse_the_same_as_lf() {
        assertEquals(kinds("# One\n- two"), kinds("# One\r\n- two"))
    }

    @Test
    fun an_empty_document_is_empty() {
        assertTrue(parseMarkdown("").isEmpty())
        assertTrue(parseMarkdown("\n\n").isEmpty())
    }
}
