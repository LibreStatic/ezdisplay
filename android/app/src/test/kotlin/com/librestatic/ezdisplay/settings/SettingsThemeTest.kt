package com.librestatic.ezdisplay.settings

import androidx.compose.material3.lightColorScheme
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.toArgb
import androidx.core.graphics.ColorUtils
import org.junit.Assert.assertEquals
import org.junit.Assert.assertSame
import org.junit.Assert.assertTrue
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner

@RunWith(RobolectricTestRunner::class)
class SettingsThemeTest {
    private fun contrast(c: androidx.compose.material3.ColorScheme) =
        ColorUtils.calculateContrast(c.onPrimary.toArgb(), c.primary.toArgb())

    @Test fun lowContrastLightPrimaryIsDarkenedToAA() {
        val samsung = lightColorScheme(primary = Color(0xFF387AFF), onPrimary = Color(0xFFFFFFFF))
        assertTrue(contrast(samsung) < 4.5)
        val fixed = ensurePrimaryContrast(samsung, dark = false)
        assertTrue("contrast ${contrast(fixed)}", contrast(fixed) >= 4.5)
        assertEquals(samsung.onPrimary, fixed.onPrimary)
    }

    @Test fun lowContrastDarkPrimaryIsLightenedToAA() {
        val scheme = androidx.compose.material3.darkColorScheme(primary = Color(0xFF3A5FA8), onPrimary = Color(0xFF000000))
        assertTrue(contrast(scheme) < 4.5)
        assertTrue(contrast(ensurePrimaryContrast(scheme, dark = true)) >= 4.5)
    }

    @Test fun whiteTextOnPaleDarkPrimaryIsDarkenedToAA() {
        val samsung = androidx.compose.material3.darkColorScheme(primary = Color(0xFFD6F4FF), onPrimary = Color(0xFFFFFFFF))
        assertTrue(contrast(samsung) < 4.5)
        val fixed = ensurePrimaryContrast(samsung, dark = true)
        assertTrue("contrast ${contrast(fixed)}", contrast(fixed) >= 4.5)
        assertEquals(samsung.primary, fixed.primary)
    }

    @Test fun seedSchemesAreUnchanged() {
        val light = ensurePrimaryContrast(SeedLight, dark = false)
        val dark = ensurePrimaryContrast(SeedDark, dark = true)
        assertSame(SeedLight, light)
        assertSame(SeedDark, dark)
    }
}
