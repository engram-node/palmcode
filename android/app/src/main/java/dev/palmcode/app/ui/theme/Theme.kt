package dev.palmcode.app.ui.theme

import android.os.Build
import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Typography
import androidx.compose.material3.darkColorScheme
import androidx.compose.material3.dynamicDarkColorScheme
import androidx.compose.material3.dynamicLightColorScheme
import androidx.compose.material3.lightColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.Immutable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.remember
import androidx.compose.runtime.staticCompositionLocalOf
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.lerp
import androidx.compose.ui.platform.LocalConfiguration
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.SpanStyle
import androidx.compose.ui.text.TextLinkStyles
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontStyle
import androidx.compose.ui.text.style.TextDecoration
import com.mikepenz.markdown.model.DefaultMarkdownColors
import com.mikepenz.markdown.model.DefaultMarkdownTypography
import com.mikepenz.markdown.model.MarkdownColors
import com.mikepenz.markdown.model.MarkdownTypography
import dev.palmcode.app.ThemeMode

// Static tonal palettes from one teal seed; no runtime palette generation.
private val Seed = Color(0xFF006A60)
private val LightColors = lightColorScheme(
    primary = Seed,
    onPrimary = Color(0xFFFFFFFF),
    primaryContainer = Color(0xFF9EF2E0),
    onPrimaryContainer = Color(0xFF00201B),
    secondary = Color(0xFF4A635D),
    onSecondary = Color(0xFFFFFFFF),
    secondaryContainer = Color(0xFFCCE8E0),
    onSecondaryContainer = Color(0xFF05201A),
    tertiary = Color(0xFF446179),
    onTertiary = Color(0xFFFFFFFF),
    tertiaryContainer = Color(0xFFCCE5FF),
    onTertiaryContainer = Color(0xFF001E31),
    error = Color(0xFFBA1A1A),
    onError = Color(0xFFFFFFFF),
    errorContainer = Color(0xFFFFDAD6),
    onErrorContainer = Color(0xFF410002),
    background = Color(0xFFF5FBF7),
    onBackground = Color(0xFF171D1B),
    surface = Color(0xFFF5FBF7),
    onSurface = Color(0xFF171D1B),
    surfaceVariant = Color(0xFFDAE5DF),
    onSurfaceVariant = Color(0xFF3F4945),
    surfaceTint = Seed,
    surfaceBright = Color(0xFFF5FBF7),
    surfaceDim = Color(0xFFD5DBD7),
    surfaceContainerLowest = Color(0xFFFFFFFF),
    surfaceContainerLow = Color(0xFFEFF5F1),
    surfaceContainer = Color(0xFFE9EFEB),
    surfaceContainerHigh = Color(0xFFE3EAE5),
    surfaceContainerHighest = Color(0xFFDEE4E0),
    outline = Color(0xFF6F7975),
    outlineVariant = Color(0xFFBEC9C3),
    inverseSurface = Color(0xFF2B322E),
    inverseOnSurface = Color(0xFFECF2EE),
    inversePrimary = Color(0xFF82D5C4),
    scrim = Color(0xFF000000),
)
private val DarkColors = darkColorScheme(
    primary = Color(0xFF82D5C4),
    onPrimary = Color(0xFF00382F),
    primaryContainer = Color(0xFF005047),
    onPrimaryContainer = Color(0xFF9EF2E0),
    secondary = Color(0xFFB0CCC4),
    onSecondary = Color(0xFF1C352E),
    secondaryContainer = Color(0xFF334B45),
    onSecondaryContainer = Color(0xFFCCE8E0),
    tertiary = Color(0xFFACCAE5),
    onTertiary = Color(0xFF143349),
    tertiaryContainer = Color(0xFF2C4961),
    onTertiaryContainer = Color(0xFFCCE5FF),
    error = Color(0xFFFFB4AB),
    onError = Color(0xFF690005),
    errorContainer = Color(0xFF93000A),
    onErrorContainer = Color(0xFFFFDAD6),
    background = Color(0xFF0E1512),
    onBackground = Color(0xFFDEE4E0),
    surface = Color(0xFF0E1512),
    onSurface = Color(0xFFDEE4E0),
    surfaceVariant = Color(0xFF3F4945),
    onSurfaceVariant = Color(0xFFBEC9C3),
    surfaceTint = Color(0xFF82D5C4),
    surfaceBright = Color(0xFF343B37),
    surfaceDim = Color(0xFF0E1512),
    surfaceContainerLowest = Color(0xFF090F0D),
    surfaceContainerLow = Color(0xFF171D1B),
    surfaceContainer = Color(0xFF1B211F),
    surfaceContainerHigh = Color(0xFF252B29),
    surfaceContainerHighest = Color(0xFF303633),
    outline = Color(0xFF89938D),
    outlineVariant = Color(0xFF3F4945),
    inverseSurface = Color(0xFFDEE4E0),
    inverseOnSurface = Color(0xFF2B322E),
    inversePrimary = Seed,
    scrim = Color(0xFF000000),
)
private val AppTypography = Typography()

@Immutable
data class ContentStyles(
    val diffAdded: Color,
    val diffRemoved: Color,
    val markdownColors: MarkdownColors,
    val markdownTypography: MarkdownTypography,
)

val LocalContentStyles = staticCompositionLocalOf<ContentStyles> {
    error("ContentStyles requires PalmcodeTheme")
}

@Composable
fun PalmcodeTheme(
    mode: ThemeMode = ThemeMode.System,
    dynamicColor: Boolean = true,
    onDarkThemeChanged: (Boolean) -> Unit = {},
    content: @Composable () -> Unit,
) {
    val dark = when (mode) {
        ThemeMode.System -> isSystemInDarkTheme()
        ThemeMode.Light -> false
        ThemeMode.Dark -> true
    }
    val context = LocalContext.current
    val configuration = LocalConfiguration.current
    val colors = remember(context, configuration, dark, dynamicColor) {
        if (dynamicColor && Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
            if (dark) dynamicDarkColorScheme(context) else dynamicLightColorScheme(context)
        } else {
            if (dark) DarkColors else LightColors
        }
    }
    LaunchedEffect(dark) { onDarkThemeChanged(dark) }
    // Build renderer styles and blend diff backgrounds once per palette,
    // above the transcript. Streaming only consumes these immutable values.
    val styles = remember(colors) {
        val type = AppTypography
        ContentStyles(
            diffAdded = lerp(colors.tertiary, colors.surfaceContainerHigh, 0.88f),
            diffRemoved = lerp(colors.error, colors.surfaceContainerHigh, 0.88f),
            markdownColors = DefaultMarkdownColors(
                text = colors.onSurface,
                codeBackground = colors.surfaceContainerHigh,
                inlineCodeBackground = colors.surfaceContainerHigh,
                dividerColor = colors.outlineVariant,
                tableBackground = colors.surfaceContainer,
            ),
            markdownTypography = DefaultMarkdownTypography(
                h1 = type.displayLarge, h2 = type.displayMedium, h3 = type.displaySmall,
                h4 = type.headlineMedium, h5 = type.headlineSmall, h6 = type.titleLarge,
                text = type.bodyLarge,
                code = type.bodyMedium.copy(fontFamily = FontFamily.Monospace, color = colors.onSurface),
                inlineCode = type.bodyLarge.copy(fontFamily = FontFamily.Monospace, color = colors.onSurface),
                quote = type.bodyMedium.copy(fontStyle = FontStyle.Italic),
                paragraph = type.bodyLarge, ordered = type.bodyLarge,
                bullet = type.bodyLarge, list = type.bodyLarge,
                textLink = TextLinkStyles(SpanStyle(color = colors.primary, textDecoration = TextDecoration.Underline)),
                table = type.bodyLarge,
            ),
        )
    }
    CompositionLocalProvider(LocalContentStyles provides styles) {
        MaterialTheme(colorScheme = colors, typography = AppTypography, content = content)
    }
}
