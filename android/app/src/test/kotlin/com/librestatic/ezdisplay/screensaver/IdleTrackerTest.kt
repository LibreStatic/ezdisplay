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

    @Test fun newFramesRestartTheIdleTimerButRepeatedTotalsDoNot() {
        val t = tracker()
        t.onFramesReceived(10, 0)
        t.onFramesReceived(11, 4 * min)
        t.onFramesReceived(11, 5 * min) // same total: no activity
        t.tick(5 * min)
        assertEquals(IdleState.Active, t.state)
        t.tick(9 * min)
        assertEquals(IdleState.Saver, t.state)
    }

    @Test fun goesBlankBlankMinutesAfterEnteringSaver() {
        val t = tracker()
        t.tick(5 * min)
        t.tick(15 * min - 1)
        assertEquals(IdleState.Saver, t.state)
        t.tick(15 * min)
        assertEquals(IdleState.Blank, t.state)
    }

    @Test fun aFrameInSaverWakesWithoutConsumingInput() {
        val t = tracker()
        t.onFramesReceived(1, 0)
        t.tick(5 * min)
        t.onFramesReceived(2, 5 * min + 1)
        assertEquals(IdleState.Active, t.state)
    }

    @Test fun inputInSaverWakesAndIsConsumed() {
        val t = tracker()
        t.tick(5 * min)
        assertTrue(t.onLocalInput(5 * min + 10))
        assertEquals(IdleState.Active, t.state)
        assertFalse(t.onLocalInput(5 * min + 20))
    }

    @Test fun framesDoNotWakeBlankButInputDoes() {
        val t = tracker()
        t.onFramesReceived(1, 0)
        t.tick(5 * min)
        t.tick(15 * min)
        t.onFramesReceived(50, 16 * min)
        assertEquals(IdleState.Blank, t.state)
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
