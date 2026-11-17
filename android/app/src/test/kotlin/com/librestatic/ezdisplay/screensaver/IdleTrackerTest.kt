package com.librestatic.ezdisplay.screensaver

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class IdleTrackerTest {
    private val min = 60_000L

    private fun config(enabled: Boolean = true, idle: Int = 5, blank: Int = 10) =
        ScreensaverConfig(enabled, idle, 10, blank)

    private fun tracker(c: ScreensaverConfig = config()) = IdleTracker(c, 0).also { it.setArmed(true, 0) }

    @Test fun staysActiveBeforeTheIdleTime() {
        val t = tracker()
        t.tick(5 * min - 1)
        assertEquals(IdleState.Active, t.state)
    }

    @Test fun goesToSaverAfterTheIdleTime() {
        val t = tracker()
        t.tick(5 * min)
        assertEquals(IdleState.Saver, t.state)
    }

    @Test fun inputRestartsTheIdleTimer() {
        val t = tracker()
        assertFalse(t.onLocalInput(4 * min))
        t.tick(5 * min)
        assertEquals(IdleState.Active, t.state)
        t.tick(9 * min)
        assertEquals(IdleState.Saver, t.state)
    }

    @Test fun framesAloneNeitherWakeNorPostponeTheSaver() {
        val t = tracker()
        assertFalse(t.onFramesReceived(10, 0)) // first count: nothing to compare with
        assertTrue(t.onFramesReceived(11, 4 * min)) // grew: the caller should measure
        assertFalse(t.onFramesReceived(11, 4 * min + 1)) // same total
        t.tick(5 * min)
        assertEquals(IdleState.Saver, t.state)
        assertTrue(t.onFramesReceived(12, 5 * min + 1))
        assertEquals(IdleState.Saver, t.state)
    }

    @Test fun framesInBlankOrWhenNotRunningAreNotMeasured() {
        val t = tracker()
        t.onFramesReceived(1, 0)
        t.tick(5 * min)
        t.tick(15 * min)
        assertFalse(t.onFramesReceived(2, 16 * min))
        val off = tracker(config(enabled = false))
        off.onFramesReceived(1, 0)
        assertFalse(off.onFramesReceived(2, 1))
    }

    @Test fun changeBelowTheThresholdIsNotActivity() {
        val t = tracker()
        assertFalse(t.onScreenChange(0.49f, 4 * min))
        t.tick(5 * min)
        assertEquals(IdleState.Saver, t.state)
        assertFalse(t.onScreenChange(0.49f, 5 * min + 1))
        assertEquals(IdleState.Saver, t.state)
    }

    @Test fun changeAtTheThresholdRestartsTheIdleTimerInActive() {
        val t = tracker()
        assertTrue(t.onScreenChange(0.5f, 4 * min))
        t.tick(5 * min)
        assertEquals(IdleState.Active, t.state)
        t.tick(9 * min)
        assertEquals(IdleState.Saver, t.state)
    }

    @Test fun changeAboveTheThresholdWakesTheSaver() {
        val t = tracker()
        t.tick(5 * min)
        assertTrue(t.onScreenChange(0.9f, 5 * min + 1))
        assertEquals(IdleState.Active, t.state)
    }

    @Test fun thresholdOf100NeedsTheWholeScreen() {
        val t = tracker(config().copy(wakePct = 100))
        t.tick(5 * min)
        assertFalse(t.onScreenChange(0.99f, 5 * min + 1))
        assertEquals(IdleState.Saver, t.state)
        assertTrue(t.onScreenChange(1f, 5 * min + 2))
        assertEquals(IdleState.Active, t.state)
    }

    @Test fun thresholdFollowsTheConfig() {
        val t = tracker(config().copy(wakePct = 5))
        t.tick(5 * min)
        assertTrue(t.onScreenChange(0.05f, 5 * min + 1))
        assertEquals(IdleState.Active, t.state)
    }

    @Test fun screenChangeDoesNotWakeBlank() {
        val t = tracker()
        t.tick(5 * min)
        t.tick(15 * min)
        assertFalse(t.onScreenChange(1f, 16 * min))
        assertEquals(IdleState.Blank, t.state)
    }

    @Test fun hostCursorWakesTheSaverAndBlankAtAnyThreshold() {
        val t = tracker(config().copy(wakePct = 100))
        t.tick(5 * min)
        t.onHostCursor(5 * min + 1)
        assertEquals(IdleState.Active, t.state)
        t.tick(10 * min + 1)
        t.tick(20 * min + 1)
        assertEquals(IdleState.Blank, t.state)
        t.onHostCursor(21 * min)
        assertEquals(IdleState.Active, t.state)
    }

    @Test fun hostCursorRestartsTheIdleTimerAndIsNotLocalInput() {
        val t = tracker()
        t.onHostCursor(4 * min)
        t.tick(5 * min)
        assertEquals(IdleState.Active, t.state)
        t.tick(9 * min)
        assertEquals(IdleState.Saver, t.state)
        t.onHostCursor(9 * min + 1)
        assertFalse(t.onLocalInput(9 * min + 2)) // nothing left to swallow
    }

    @Test fun disabledOrDisarmedTrackerIgnoresContentAndCursor() {
        val off = tracker(config(enabled = false))
        assertFalse(off.onScreenChange(1f, 1))
        off.onHostCursor(1)
        assertEquals(IdleState.Active, off.state)
        val t = tracker()
        t.tick(5 * min)
        t.setArmed(false, 6 * min)
        assertFalse(t.onScreenChange(1f, 7 * min))
        t.tick(100 * min)
        t.onHostCursor(101 * min)
        assertEquals(IdleState.Active, t.state)
    }

    @Test fun goesBlankBlankMinutesAfterEnteringSaver() {
        val t = tracker()
        t.tick(5 * min)
        t.tick(15 * min - 1)
        assertEquals(IdleState.Saver, t.state)
        t.tick(15 * min)
        assertEquals(IdleState.Blank, t.state)
    }

    @Test fun inputInSaverWakesAndIsConsumed() {
        val t = tracker()
        t.tick(5 * min)
        assertTrue(t.onLocalInput(5 * min + 10))
        assertEquals(IdleState.Active, t.state)
        assertFalse(t.onLocalInput(5 * min + 20))
    }

    @Test fun inputWakesBlank() {
        val t = tracker()
        t.tick(5 * min)
        t.tick(15 * min)
        assertTrue(t.onLocalInput(17 * min))
        assertEquals(IdleState.Active, t.state)
    }

    @Test fun blankMinZeroNeverBlanks() {
        val t = tracker(config(blank = 0))
        t.tick(5 * min)
        t.tick(1000 * min)
        assertEquals(IdleState.Saver, t.state)
    }

    @Test fun disabledConfigStaysActiveAndDoesNotConsumeInput() {
        val t = tracker(config(enabled = false))
        t.tick(1000 * min)
        assertEquals(IdleState.Active, t.state)
        assertFalse(t.onLocalInput(1000 * min))
    }

    @Test fun disarmingResetsToActiveAndRestartsTimers() {
        val t = tracker()
        t.tick(5 * min)
        t.setArmed(false, 6 * min)
        assertEquals(IdleState.Active, t.state)
        t.tick(100 * min)
        assertEquals(IdleState.Active, t.state)
        t.setArmed(true, 100 * min)
        t.tick(104 * min)
        assertEquals(IdleState.Active, t.state)
        t.tick(105 * min)
        assertEquals(IdleState.Saver, t.state)
    }

    @Test fun newConfigRestartsTimers() {
        val t = tracker()
        t.tick(5 * min)
        t.setConfig(config(idle = 10), 6 * min)
        assertEquals(IdleState.Active, t.state)
        t.tick(15 * min)
        assertEquals(IdleState.Active, t.state)
        t.tick(16 * min)
        assertEquals(IdleState.Saver, t.state)
    }
}
