package com.librestatic.ezdisplay.screensaver

import androidx.compose.ui.graphics.Matrix
import androidx.compose.ui.graphics.Path
import androidx.compose.ui.graphics.vector.PathParser

/**
 * Silhouettes the screensaver flies around: a monitor, a tablet, a stylus, a brush, a palette and a mouse.
 * Path data of the filled Material Symbols Rounded icons (Apache-2.0, @material-symbols/svg-400 0.48.1), in their
 * 960-unit viewport whose y runs from -960 to 0.
 */
private val GlyphPaths = listOf(
    // desktop_windows
    "M420-180v-100H140q-24 0-42-18t-18-42v-440q0-24 18-42t42-18h680q24 0 42 18t18 42v440q0 24-18 42t-42 18H540v100h56q13 0 21.5 8.5T626-150q0 13-8.5 21.5T596-120H364q-13 0-21.5-8.5T334-150q0-13 8.5-21.5T364-180h56Z",
    // tablet_android
    "M180-40q-24.75 0-42.37-17.63Q120-75.25 120-100v-760q0-24.75 17.63-42.38Q155.25-920 180-920h600q24.75 0 42.38 17.62Q840-884.75 840-860v760q0 24.75-17.62 42.37Q804.75-40 780-40H180Zm0-210h600v-520H180v520Zm248 125h124q8 0 14-6t6-14q0-8-6-14t-14-6H428q-8 0-14 6t-6 14q0 8 6 14t14 6Z",
    // stylus
    "M167-120q-21 5-36.5-10.5T120-167l36-169 180 180-169 36Zm221-70L190-388l443-443q17-17 42-17t42 17l114 114q17 17 17 42t-17 42L388-190Z",
    // brush
    "M215-117q-34 0-67-11.5T90-166q35-12 50-35t15-62q0-44 30.5-74.5T260-368q44 0 74.5 30.5T365-263q0 64-43.5 105T215-117Zm230-237-90-95 376-376q14-14 31-14.5t32 14.5l29 29q15 15 14.5 32.5T823-732L445-354Z",
    // palette
    "M480-80q-82 0-155-31.5t-127.5-86Q143-252 111.5-325T80-480q0-84 32-157t87.5-127q55.5-54 130-85T489-880q79 0 150 26.5T763.5-780q53.5 47 85 111.5T880-527q0 108-63 170.5T650-294h-75q-18 0-31 14t-13 31q0 20 14.5 38t14.5 43q0 26-24.5 57T480-80ZM282-469q15-15 15-35t-15-35q-15-15-35-15t-35 15q-15 15-15 35t15 35q15 15 35 15t35-15Zm126-170q15-15 15-35t-15-35q-15-15-35-15t-35 15q-15 15-15 35t15 35q15 15 35 15t35-15Zm214 0q15-15 15-35t-15-35q-15-15-35-15t-35 15q-15 15-15 35t15 35q15 15 35 15t35-15Zm131 170q15-15 15-35t-15-35q-15-15-35-15t-35 15q-15 15-15 35t15 35q15 15 35 15t35-15Z",
    // mouse
    "M480-80q-117 0-198.5-81.5T200-360v-200h560v200q0 117-81.5 198.5T480-80ZM200-620q0-108 71.5-187.5T450-898v278H200Zm310 0v-278q107 11 178.5 90.5T760-620H510Z",
)

/** Number of distinct silhouettes; a body's shape index is taken modulo this. */
val ScreensaverGlyphCount: Int = GlyphPaths.size

/** Silhouette [index] (modulo the count) scaled to fill a [side] by [side] px square with its origin at the top left. */
fun screensaverGlyph(index: Int, side: Float): Path {
    val path = PathParser().parsePathString(GlyphPaths[index % GlyphPaths.size]).toPath()
    path.transform(
        Matrix().apply {
            scale(side / 960f, side / 960f)
            translate(0f, 960f)
        },
    )
    return path
}
