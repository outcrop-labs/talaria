package app.talaria.mobile.instance

import io.ktor.http.URLProtocol
import io.ktor.http.Url

/**
 * An instance IS its origin. This is the Kotlin half of a contract the desktop
 * shell already states in Rust (`desktop/src-tauri/src/beacon.rs`
 * `normalize_origin`), and the two must agree: the same typed URL has to
 * produce the same registry key on a phone as it does on a laptop, or the same
 * instance shows up twice in one person's life.
 *
 * The rules, verbatim from that file: bare hosts get `https://` assumed; the
 * scheme must be http or https; paths, queries, fragments and credentials are
 * stripped. Default ports collapse, so `https://x:443` and `https://x` are one
 * instance rather than two.
 */
sealed interface OriginResult {
    data class Ok(val origin: String) : OriginResult

    data class Invalid(val reason: String) : OriginResult
}

/** Hosts we will accept. Ktor's parser is lenient by design — it will hand back
 *  a `Url` with a space in the host rather than refusing — so the host is
 *  checked here instead of trusted. Letters, digits, dot, dash, underscore, and
 *  the brackets/colons an IPv6 literal needs. */
private val HOST = Regex("^[A-Za-z0-9._\\-]+$|^\\[[0-9A-Fa-f:.]+]$")

fun normalizeOrigin(input: String): OriginResult {
    val trimmed = input.trim()
    if (trimmed.isEmpty()) return OriginResult.Invalid("the URL has no host")

    // Interior whitespace is refused BEFORE parsing, and this is the first
    // place Ktor and Rust's `url` crate disagree. `Url::parse` rejects
    // "https://not a url at all" outright; Ktor takes the host up to the space
    // and hands back a perfectly usable Url for `https://not`. Without this
    // line, typing a sentence into the field adds an instance called "not".
    // (The shared test list caught exactly this on its first run.)
    if (trimmed.any { it.isWhitespace() }) return OriginResult.Invalid("not a valid URL")

    val candidate = if (trimmed.contains("://")) trimmed else "https://$trimmed"

    // The authority is read out of the INPUT, not out of the parsed Url, and
    // this is the second place Ktor's leniency differs from Rust's `url`.
    // `URLBuilder` defaults an empty host to "localhost", so Ktor turns a bare
    // "https://" into a perfectly valid `https://localhost` — meaning the
    // parsed object can never tell us the host was missing. `Url::parse`
    // refuses it. Checking the string is the only way to agree with that.
    val authority = candidate.substringAfter("://").takeWhile { it != '/' && it != '?' && it != '#' }
    if (authority.isEmpty()) return OriginResult.Invalid("the URL has no host")

    val url =
        try {
            Url(candidate)
        } catch (_: Throwable) {
            return OriginResult.Invalid("not a valid URL")
        }

    val scheme = url.protocol.name
    if (scheme != "http" && scheme != "https") {
        return OriginResult.Invalid("“$scheme” is not an http(s) URL")
    }
    if (!url.user.isNullOrEmpty() || !url.password.isNullOrEmpty()) {
        return OriginResult.Invalid("URLs with embedded credentials are not accepted")
    }
    if (url.host.isBlank() || !HOST.matches(url.host)) {
        return OriginResult.Invalid("the URL has no host")
    }

    // A port is part of the origin only when it is not the scheme's default:
    // dropping it is what makes https://x and https://x:443 the same instance.
    val port = url.specifiedPort.takeIf { it > 0 && it != url.protocol.defaultPort }
    return OriginResult.Ok(buildString {
        append(scheme).append("://").append(url.host)
        if (port != null) append(':').append(port)
    })
}

/** The fallback label for an instance whose beacon carries no company name:
 *  host, with the port only when it is not the default — the same shape the
 *  desktop launcher's list shows. */
fun hostLabel(origin: String): String =
    when (val r = normalizeOrigin(origin)) {
        is OriginResult.Invalid -> origin
        is OriginResult.Ok ->
            r.origin
                .removePrefix("${URLProtocol.HTTPS.name}://")
                .removePrefix("${URLProtocol.HTTP.name}://")
    }
