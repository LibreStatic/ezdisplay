package com.librestatic.ezdisplay.screensaver

import org.junit.Assert.assertTrue
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner

@RunWith(RobolectricTestRunner::class)
class ScreensaverGlyphsTest {
    @Test fun everyGlyphFillsMostOfItsSquareAndStaysInside() {
        val side = 200f
        repeat(ScreensaverGlyphCount) { i ->
            val b = screensaverGlyph(i, side).getBounds()
            assertTrue("glyph $i bounds $b", b.left >= -0.5f && b.top >= -0.5f && b.right <= side + 0.5f && b.bottom <= side + 0.5f)
            assertTrue("glyph $i is too small: $b", b.width >= side * 0.6f || b.height >= side * 0.6f)
        }
    }
}
