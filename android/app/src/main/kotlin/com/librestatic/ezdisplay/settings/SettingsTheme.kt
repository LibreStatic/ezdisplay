package com.librestatic.ezdisplay.settings

import android.content.Context
import android.os.Build
import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.material3.ExperimentalMaterial3ExpressiveApi
import androidx.compose.material3.ColorScheme
import androidx.compose.material3.MaterialExpressiveTheme
import androidx.compose.material3.MotionScheme
import androidx.compose.material3.darkColorScheme
import androidx.compose.material3.dynamicDarkColorScheme
import androidx.compose.material3.dynamicLightColorScheme
import androidx.compose.material3.lightColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.toArgb
import androidx.compose.ui.platform.LocalContext
import androidx.core.graphics.ColorUtils

/** Material 3 Expressive theme: Material You colors on API 31+ when [dynamicColor], the seed schemes otherwise. */
@OptIn(ExperimentalMaterial3ExpressiveApi::class)
@Composable
fun SettingsTheme(dynamicColor: Boolean = true, content: @Composable () -> Unit) {
    val dark = isSystemInDarkTheme()
    val context: Context = LocalContext.current
    val colorScheme = when {
        dynamicColor && Build.VERSION.SDK_INT >= Build.VERSION_CODES.S ->
            ensurePrimaryContrast(if (dark) dynamicDarkColorScheme(context) else dynamicLightColorScheme(context), dark)
        dark -> SeedDark
        else -> SeedLight
    }
    MaterialExpressiveTheme(colorScheme = colorScheme, motionScheme = MotionScheme.expressive(), content = content)
}

private const val MinContrast = 4.5

/**
 * Some OEM dynamic schemes pair `primary` and `onPrimary` below WCAG AA. Samsung's light scheme has #387AFF on
 * #FFFFFF (3.9:1); its dark scheme has a pale `primary` with a white `onPrimary`, which makes filled buttons unreadable.
 * Walks CIELAB lightness (keeping a and b) until the pair reaches 4.5:1: `primary` down in light themes; in dark themes
 * `onPrimary` down when it is lighter than `primary`, `primary` up otherwise. Schemes that already pass are unchanged.
 */
internal fun ensurePrimaryContrast(scheme: ColorScheme, dark: Boolean): ColorScheme {
    val on = scheme.onPrimary.toArgb()
    val primary = scheme.primary.toArgb()
    if (ColorUtils.calculateContrast(on, primary) >= MinContrast) return scheme
    if (dark && ColorUtils.calculateLuminance(on) > ColorUtils.calculateLuminance(primary)) {
        return scheme.copy(onPrimary = Color(walkLightness(on, against = primary, step = -1.0)))
    }
    return scheme.copy(primary = Color(walkLightness(primary, against = on, step = if (dark) 1.0 else -1.0)))
}

/** Moves [color]'s CIELAB lightness by [step] until it reaches [MinContrast] against [against] or runs out of range. */
private fun walkLightness(color: Int, against: Int, step: Double): Int {
    val lab = DoubleArray(3)
    ColorUtils.colorToLAB(color, lab)
    var l = lab[0]
    var adjusted = color
    while (ColorUtils.calculateContrast(against, adjusted) < MinContrast) {
        l += step
        if (l !in 0.0..100.0) break
        adjusted = ColorUtils.LABToColor(l, lab[1], lab[2])
    }
    return adjusted
}

// Generated from seed #2C2A79 with material-color-utilities (tonal spot).
internal val SeedLight = lightColorScheme(
    primary = Color(0xFF595992),
    onPrimary = Color(0xFFFFFFFF),
    primaryContainer = Color(0xFFE2DFFF),
    onPrimaryContainer = Color(0xFF414178),
    inversePrimary = Color(0xFFC2C1FF),
    secondary = Color(0xFF5D5C71),
    onSecondary = Color(0xFFFFFFFF),
    secondaryContainer = Color(0xFFE3E0F9),
    onSecondaryContainer = Color(0xFF454559),
    tertiary = Color(0xFF7A5368),
    onTertiary = Color(0xFFFFFFFF),
    tertiaryContainer = Color(0xFFFFD8EB),
    onTertiaryContainer = Color(0xFF603C50),
    background = Color(0xFFFCF8FF),
    onBackground = Color(0xFF1B1B21),
    surface = Color(0xFFFCF8FF),
    onSurface = Color(0xFF1B1B21),
    surfaceVariant = Color(0xFFE4E1EC),
    onSurfaceVariant = Color(0xFF47464F),
    surfaceTint = Color(0xFF595992),
    inverseSurface = Color(0xFF303036),
    inverseOnSurface = Color(0xFFF3EFF7),
    error = Color(0xFFBA1A1A),
    onError = Color(0xFFFFFFFF),
    errorContainer = Color(0xFFFFDAD6),
    onErrorContainer = Color(0xFF93000A),
    outline = Color(0xFF787680),
    outlineVariant = Color(0xFFC8C5D0),
    scrim = Color(0xFF000000),
    surfaceBright = Color(0xFFFCF8FF),
    surfaceContainer = Color(0xFFF0ECF4),
    surfaceContainerHigh = Color(0xFFEAE7EF),
    surfaceContainerHighest = Color(0xFFE5E1E9),
    surfaceContainerLow = Color(0xFFF6F2FA),
    surfaceContainerLowest = Color(0xFFFFFFFF),
    surfaceDim = Color(0xFFDCD9E0),
    primaryFixed = Color(0xFFE2DFFF),
    primaryFixedDim = Color(0xFFC2C1FF),
    onPrimaryFixed = Color(0xFF15134A),
    onPrimaryFixedVariant = Color(0xFF414178),
    secondaryFixed = Color(0xFFE3E0F9),
    secondaryFixedDim = Color(0xFFC6C4DD),
    onSecondaryFixed = Color(0xFF1A1A2C),
    onSecondaryFixedVariant = Color(0xFF454559),
    tertiaryFixed = Color(0xFFFFD8EB),
    tertiaryFixedDim = Color(0xFFEAB9D2),
    onTertiaryFixed = Color(0xFF2F1124),
    onTertiaryFixedVariant = Color(0xFF603C50),
)

internal val SeedDark = darkColorScheme(
    primary = Color(0xFFC2C1FF),
    onPrimary = Color(0xFF2B2A60),
    primaryContainer = Color(0xFF414178),
    onPrimaryContainer = Color(0xFFE2DFFF),
    inversePrimary = Color(0xFF595992),
    secondary = Color(0xFFC6C4DD),
    onSecondary = Color(0xFF2F2F42),
    secondaryContainer = Color(0xFF454559),
    onSecondaryContainer = Color(0xFFE3E0F9),
    tertiary = Color(0xFFEAB9D2),
    onTertiary = Color(0xFF472639),
    tertiaryContainer = Color(0xFF603C50),
    onTertiaryContainer = Color(0xFFFFD8EB),
    background = Color(0xFF131318),
    onBackground = Color(0xFFE5E1E9),
    surface = Color(0xFF131318),
    onSurface = Color(0xFFE5E1E9),
    surfaceVariant = Color(0xFF47464F),
    onSurfaceVariant = Color(0xFFC8C5D0),
    surfaceTint = Color(0xFFC2C1FF),
    inverseSurface = Color(0xFFE5E1E9),
    inverseOnSurface = Color(0xFF303036),
    error = Color(0xFFFFB4AB),
    onError = Color(0xFF690005),
    errorContainer = Color(0xFF93000A),
    onErrorContainer = Color(0xFFFFDAD6),
    outline = Color(0xFF918F9A),
    outlineVariant = Color(0xFF47464F),
    scrim = Color(0xFF000000),
    surfaceBright = Color(0xFF39383F),
    surfaceContainer = Color(0xFF1F1F25),
    surfaceContainerHigh = Color(0xFF2A292F),
    surfaceContainerHighest = Color(0xFF35343A),
    surfaceContainerLow = Color(0xFF1B1B21),
    surfaceContainerLowest = Color(0xFF0E0E13),
    surfaceDim = Color(0xFF131318),
    primaryFixed = Color(0xFFE2DFFF),
    primaryFixedDim = Color(0xFFC2C1FF),
    onPrimaryFixed = Color(0xFF15134A),
    onPrimaryFixedVariant = Color(0xFF414178),
    secondaryFixed = Color(0xFFE3E0F9),
    secondaryFixedDim = Color(0xFFC6C4DD),
    onSecondaryFixed = Color(0xFF1A1A2C),
    onSecondaryFixedVariant = Color(0xFF454559),
    tertiaryFixed = Color(0xFFFFD8EB),
    tertiaryFixedDim = Color(0xFFEAB9D2),
    onTertiaryFixed = Color(0xFF2F1124),
    onTertiaryFixedVariant = Color(0xFF603C50),
)

