package app.talaria.mobile.instance

import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertIs
import kotlin.test.assertTrue

/**
 * These cases are deliberately the SAME cases as
 * `desktop/src-tauri/src/beacon.rs`'s test module, answer for answer. The two
 * implementations are independent, so only a shared test list keeps them
 * agreeing — and if they disagree, one instance acquires two identities in one
 * person's account list.
 */
class OriginTest {
    private fun ok(input: String): String {
        val r = normalizeOrigin(input)
        assertIs<OriginResult.Ok>(r, "expected $input to normalize")
        return r.origin
    }

    private fun rejected(input: String) {
        assertIs<OriginResult.Invalid>(normalizeOrigin(input), "expected $input to be refused")
    }

    @Test
    fun bare_hosts_become_https() {
        assertEquals("https://outcrop.example", ok("outcrop.example"))
    }

    @Test
    fun scheme_host_port_survive_but_nothing_else() {
        assertEquals("http://127.0.0.1:5302", ok("http://127.0.0.1:5302/some/path?x=1#frag"))
    }

    @Test
    fun non_http_schemes_are_refused() {
        rejected("ftp://example.com")
        rejected("file:///etc/passwd")
    }

    @Test
    fun embedded_credentials_are_refused() {
        rejected("https://user:pass@example.com")
    }

    @Test
    fun garbage_is_refused() {
        rejected("not a url at all")
        rejected("https://")
        rejected("")
        rejected("   ")
    }

    @Test
    fun surrounding_whitespace_is_trimmed() {
        assertEquals("https://outcrop.example", ok("  outcrop.example  "))
    }

    /** The port rule is what makes two spellings one instance. */
    @Test
    fun default_ports_collapse_but_others_do_not() {
        assertEquals("https://example.com", ok("https://example.com:443"))
        assertEquals("http://example.com", ok("http://example.com:80"))
        assertEquals("https://example.com:8443", ok("https://example.com:8443"))
    }

    @Test
    fun a_trailing_slash_is_not_part_of_the_origin() {
        assertEquals("https://example.com", ok("https://example.com/"))
    }

    @Test
    fun host_label_drops_the_scheme_but_keeps_a_nondefault_port() {
        assertEquals("127.0.0.1:5302", hostLabel("http://127.0.0.1:5302"))
        assertEquals("example.com", hostLabel("https://example.com"))
    }

    @Test
    fun normalizing_is_idempotent() {
        val once = ok("TALARIA.example.com/path")
        assertEquals(once, ok(once))
        assertTrue(once.startsWith("https://"))
    }
}
