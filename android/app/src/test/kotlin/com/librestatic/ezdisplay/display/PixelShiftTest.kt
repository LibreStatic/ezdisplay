package com.librestatic.ezdisplay.display

import android.content.SharedPreferences
import androidx.test.core.app.ApplicationProvider
import com.librestatic.ezdisplay.InputPrefs
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Before
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import kotlin.math.abs

private const val STEP = PixelShift.STEP_INTERVAL_MS
private const val QUIET = PixelShift.QUIET_MS

class PixelShiftTest {
    private fun on(r: Int = 4) = PixelShiftConfig(enabled = true, radiusPx = r)

    private fun PixelShift.pos() = offsetX to offsetY

    /** Takes one step per interval, with no input, and returns the position after each. */
    private fun walk(shift: PixelShift, steps: Int, startMs: Long = 0L): List<Pair<Int, Int>> {
        var now = startMs
        return List(steps) {
            now += STEP
            assertTrue(shift.tick(now, covered = false))
            shift.pos()
        }
    }

    @Test fun startsAtTheCentre() {
        assertEquals(0 to 0, PixelShift(on()).pos())
    }

    @Test fun staysWithinTheRadiusOverSeveralCycles() {
        for (r in listOf(1, 2, 4, 8)) {
            val n = 2 * r + 1
            for ((x, y) in walk(PixelShift(on(r)), n * n * 4)) {
                assertTrue(abs(x) <= r && abs(y) <= r)
            }
        }
    }

    @Test fun everyStepChangesOneAxisByOne() {
        var prev = 0 to 0
        for (p in walk(PixelShift(on(3)), 7 * 7 * 3)) {
            assertEquals(1, abs(p.first - prev.first) + abs(p.second - prev.second))
            prev = p
        }
    }

    @Test fun anEndToEndPassVisitsEveryPosition() {
        for (r in listOf(1, 4)) {
            val n = 2 * r + 1
            // Out to one end, back through the centre to the other end: two passes cover the grid.
            val seen = walk(PixelShift(on(r)), 2 * n * n).toSet() + (0 to 0)
            assertEquals(n * n, seen.size)
        }
    }

    @Test fun noStepBeforeTheIntervalOrWhileAPointerIsActive() {
        val s = PixelShift(on())
        assertFalse(s.tick(STEP - 1, covered = false))
        s.onTouch(true, STEP)
        assertFalse(s.tick(STEP + QUIET + 10_000, covered = false))
        assertEquals(0 to 0, s.pos())
    }

    @Test fun noStepUntilTheQuietTimeAfterTheLastPointerEvent() {
        val s = PixelShift(on())
        s.onTouch(false, STEP)
        assertFalse(s.tick(STEP + QUIET - 1, covered = false))
        assertTrue(s.tick(STEP + QUIET, covered = false))
    }

    @Test fun aPendingStepFiresOnceAndStepsDoNotAccumulate() {
        val s = PixelShift(on())
        s.onTouch(true, 0)
        assertFalse(s.tick(10 * STEP, covered = false))
        s.onTouch(false, 10 * STEP)
        assertTrue(s.tick(10 * STEP + QUIET, covered = false))
        assertEquals(1, abs(s.offsetX) + abs(s.offsetY))
        // The next step is a full interval after the one just taken.
        assertFalse(s.tick(10 * STEP + QUIET + STEP - 1, covered = false))
        assertTrue(s.tick(10 * STEP + QUIET + STEP, covered = false))
    }

    @Test fun mouseActivityDoesNotReleaseAHoveringPen() {
        val s = PixelShift(on())
        s.onHover(true, 0)
        s.onActivity(STEP)
        assertFalse(s.tick(STEP + QUIET + 10_000, covered = false))
    }

    @Test fun mouseActivityOnlyDelaysTheStep() {
        val s = PixelShift(on())
        s.onActivity(STEP)
        assertFalse(s.tick(STEP + QUIET - 1, covered = false))
        assertTrue(s.tick(STEP + QUIET, covered = false))
    }

    @Test fun mouseActivityDoesNotReleaseAHeldTouch() {
        val s = PixelShift(on())
        s.onTouch(true, 0)
        s.onActivity(STEP)
        assertFalse(s.tick(STEP + QUIET + 10_000, covered = false))
        s.onTouch(false, 2 * STEP)
        assertTrue(s.tick(2 * STEP + QUIET, covered = false))
    }

    @Test fun aHoveringPenHoldsThePictureUntilItLeaves() {
        val s = PixelShift(on())
        s.onHover(true, STEP)
        assertFalse(s.tick(STEP + QUIET + 10_000, covered = false))
        // A touch that ends while the pen is still in range does not release it either.
        s.onTouch(true, 2 * STEP)
        s.onTouch(false, 2 * STEP + 1)
        assertFalse(s.tick(3 * STEP, covered = false))
        s.onHover(false, 3 * STEP)
        assertTrue(s.tick(3 * STEP + QUIET, covered = false))
    }

    @Test fun coveredStepsDespiteRecentInput() {
        val s = PixelShift(on())
        s.onTouch(true, STEP)
        assertTrue(s.tick(STEP, covered = true))
    }

    @Test fun changingTheRadiusOrDisablingReturnsToTheCentre() {
        val s = PixelShift(on(4))
        walk(s, 5)
        assertTrue(s.offsetX != 0 || s.offsetY != 0)
        assertTrue(s.setConfig(on(2), 1_000_000L))
        assertEquals(0 to 0, s.pos())
        walk(s, 3, startMs = 1_000_000L)
        assertTrue(s.setConfig(PixelShiftConfig(enabled = false, radiusPx = 2), 2_000_000L))
        assertEquals(0 to 0, s.pos())
        assertFalse(s.setConfig(PixelShiftConfig(enabled = false, radiusPx = 2), 2_000_001L))
    }

    @Test fun theSameConfigChangesNothingAndAResetRestartsTheTimer() {
        val s = PixelShift(on())
        assertFalse(s.setConfig(on(), 5L))
        walk(s, 1)
        assertFalse(s.setConfig(on(), STEP + 1))
        assertTrue(s.setConfig(on(3), 10 * STEP))
        assertFalse(s.tick(11 * STEP - 1, covered = true))
        assertTrue(s.tick(11 * STEP, covered = true))
    }

    @Test fun disabledNeverMoves() {
        val s = PixelShift(PixelShiftConfig(enabled = false))
        for (i in 1..10) assertFalse(s.tick(i * STEP, covered = true))
        assertEquals(0 to 0, s.pos())
    }
}

@RunWith(RobolectricTestRunner::class)
class PixelShiftPrefsTest {
    private lateinit var prefs: SharedPreferences

    @Before fun setUp() {
        prefs = InputPrefs.prefs(ApplicationProvider.getApplicationContext())
        prefs.edit().clear().commit()
    }

    @Test fun defaultsFollowThePanel() {
        assertTrue(PixelShiftPrefs.read(prefs, PanelType.Oled).enabled)
        assertFalse(PixelShiftPrefs.read(prefs, PanelType.Lcd).enabled)
        assertFalse(PixelShiftPrefs.read(prefs, PanelType.Unknown).enabled)
        assertEquals(PixelShiftConfig(true, 4), PixelShiftPrefs.read(prefs, PanelType.Oled))
    }

    @Test fun aStoredValueWins() {
        prefs.edit().putBoolean(InputPrefs.PIXEL_SHIFT_ENABLED, false).commit()
        assertFalse(PixelShiftPrefs.read(prefs, PanelType.Oled).enabled)
        prefs.edit().putBoolean(InputPrefs.PIXEL_SHIFT_ENABLED, true).commit()
        assertTrue(PixelShiftPrefs.read(prefs, PanelType.Lcd).enabled)
    }

    @Test fun theRadiusIsClamped() {
        prefs.edit().putInt(InputPrefs.PIXEL_SHIFT_PX, 99).commit()
        assertEquals(8, PixelShiftPrefs.radiusPx(prefs))
        prefs.edit().putInt(InputPrefs.PIXEL_SHIFT_PX, 0).commit()
        assertEquals(1, PixelShiftPrefs.read(prefs, PanelType.Lcd).radiusPx)
    }
}
