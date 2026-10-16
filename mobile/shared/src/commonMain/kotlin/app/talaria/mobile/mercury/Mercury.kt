package app.talaria.mobile.mercury

import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.darkColorScheme
import androidx.compose.material3.lightColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.ui.graphics.Color

/**
 * Mercury, as Compose sees it.
 *
 * These are not new colors: every value is transcribed from
 * `docs/design/mercury-spec.md`, which states them as exact hex. The spec is the
 * source — when this file and that page disagree, the page wins. Dark is the
 * default theme there, and so it is here.
 *
 * The named roles are kept alongside the Material scheme on purpose. Material's
 * vocabulary (primary/surface/onSurface) cannot express "hairline strong" or
 * "ink dim", and the spec's rules are written in its own words — "orange is
 * never decorative", "primary CTAs = gold fill; destructive = orange outline".
 * Collapsing those into Material slots would lose the rules with the names.
 */
object Mercury {
    object Dark {
        val ground = Color(0xFF090A09)
        val panel = Color(0xFF141312)
        val raised = Color(0xFF1E1C1A)
        val hover = Color(0xFF24221F)
        val hairline = Color(0xFF302D29)
        val hairlineStrong = Color(0xFF4A4640)
        val readout = Color(0xFFE7E2DB)
        val inkDim = Color(0xFF5F5A53)
        val muted = Color(0xFF8E877E)
        val success = Color(0xFFA0CA92)
        val warning = Color(0xFFC8B46C)
        val danger = Color(0xFFEE6018)
        val chartBlue = Color(0xFF68B6C8)
        val chartCoral = Color(0xFFD77968)
    }

    object Light {
        val ground = Color(0xFFF2F0EB)
        val panel = Color(0xFFFAF8F4)
        val raised = Color(0xFFFFFFFF)
        val hover = Color(0xFFF2F0EB)
        val hairline = Color(0xFFDCD6CC)
        val hairlineStrong = Color(0xFFC9C2B6)
        val readout = Color(0xFF1B1917)
        val inkDim = Color(0xFF8A8378)
        val muted = Color(0xFF6E675D)
        val success = Color(0xFF5E7F4E)
        val warning = Color(0xFF8F7A33)
        val danger = Color(0xFFC8500F)
        val chartBlue = Color(0xFF3E7A8A)
        val chartCoral = Color(0xFFB05A4A)
    }

    /** The accent and the warning color are the SAME warm gold — one hex anchors
     *  the brand (spec §1). Gold fills take dark glyphs, never light ones. */
    val accent = Dark.warning
    val onAccent = Dark.ground
}

private val darkScheme =
    darkColorScheme(
        primary = Mercury.accent,
        onPrimary = Mercury.onAccent,
        background = Mercury.Dark.ground,
        onBackground = Mercury.Dark.readout,
        surface = Mercury.Dark.panel,
        onSurface = Mercury.Dark.readout,
        surfaceVariant = Mercury.Dark.raised,
        onSurfaceVariant = Mercury.Dark.muted,
        outline = Mercury.Dark.hairline,
        outlineVariant = Mercury.Dark.hairlineStrong,
        error = Mercury.Dark.danger,
    )

private val lightScheme =
    lightColorScheme(
        primary = Mercury.Light.warning,
        onPrimary = Mercury.Light.raised,
        background = Mercury.Light.ground,
        onBackground = Mercury.Light.readout,
        surface = Mercury.Light.panel,
        onSurface = Mercury.Light.readout,
        surfaceVariant = Mercury.Light.raised,
        onSurfaceVariant = Mercury.Light.muted,
        outline = Mercury.Light.hairline,
        outlineVariant = Mercury.Light.hairlineStrong,
        error = Mercury.Light.danger,
    )

@Composable
fun MercuryTheme(
    dark: Boolean = true,
    content: @Composable () -> Unit,
) = MaterialTheme(colorScheme = if (dark) darkScheme else lightScheme, content = content)
