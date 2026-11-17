package com.librestatic.ezdisplay.screensaver

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

class ScreenChangeDetectorTest {
    private fun solid(rgb: Int, n: Int = 100) = IntArray(n) { 0xFF000000.toInt() or rgb }

    @Test fun firstSampleHasNoReference() {
        val d = ScreenChangeDetector()
        assertNull(d.changedFraction(solid(0x102030)))
        assertEquals(0f, d.changedFraction(solid(0x102030))!!, 0f)
    }

    @Test fun identicalSamplesChangeNothing() {
        val d = ScreenChangeDetector()
        d.changedFraction(solid(0x808080))
        assertEquals(0f, d.changedFraction(solid(0x808080))!!, 0f)
    }

    @Test fun noiseWithinTheToleranceIsIgnored() {
        val d = ScreenChangeDetector()
        d.changedFraction(solid(0x808080))
        val noisy = solid(0x808080 + 0x180000 + 0x001800 + 0x000018) // +24 on every channel
        assertEquals(0f, d.changedFraction(noisy)!!, 0f)
        val past = solid(0x808080 + 0x190000) // +25 on red
        assertEquals(1f, d.changedFraction(past)!!, 0f)
    }

    @Test fun halfTheCellsChanged() {
        val d = ScreenChangeDetector()
        d.changedFraction(solid(0x000000))
        val sample = solid(0x000000)
        for (i in 0 until 50) sample[i] = 0xFFFFFFFF.toInt()
        assertEquals(0.5f, d.changedFraction(sample)!!, 0f)
    }

    @Test fun anySingleChannelCounts() {
        val d = ScreenChangeDetector()
        d.changedFraction(solid(0x000000, 10))
        val sample = solid(0x000000, 10)
        sample[0] = 0xFF000000.toInt() or 0x0000FF // blue only
        sample[1] = 0xFF000000.toInt() or 0x00FF00 // green only
        assertEquals(0.2f, d.changedFraction(sample)!!, 1e-6f)
    }

    @Test fun referenceMovesOnlyOnAccept() {
        val d = ScreenChangeDetector()
        d.changedFraction(solid(0x000000))
        val other = solid(0xFFFFFF)
        assertEquals(1f, d.changedFraction(other)!!, 0f)
        assertEquals(1f, d.changedFraction(other)!!, 0f) // still compared with the first one
        d.accept(other)
        assertEquals(0f, d.changedFraction(other)!!, 0f)
        assertEquals(1f, d.changedFraction(solid(0x000000))!!, 0f)
    }

    @Test fun acceptedSampleIsCopied() {
        val d = ScreenChangeDetector()
        val s = solid(0x000000)
        d.changedFraction(s)
        d.accept(s)
        s.fill(0xFFFFFFFF.toInt())
        assertEquals(1f, d.changedFraction(s)!!, 0f)
    }

    @Test fun resetForgetsTheReference() {
        val d = ScreenChangeDetector()
        d.changedFraction(solid(0x000000))
        d.reset()
        assertNull(d.changedFraction(solid(0xFFFFFF)))
        assertEquals(0f, d.changedFraction(solid(0xFFFFFF))!!, 0f)
    }
}
