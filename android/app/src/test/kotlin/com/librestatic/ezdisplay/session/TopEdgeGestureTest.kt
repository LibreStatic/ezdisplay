package com.librestatic.ezdisplay.session

import com.librestatic.ezdisplay.session.TopEdgeGesture.Companion.CANCEL
import com.librestatic.ezdisplay.session.TopEdgeGesture.Companion.DOWN
import com.librestatic.ezdisplay.session.TopEdgeGesture.Companion.MOVE
import com.librestatic.ezdisplay.session.TopEdgeGesture.Companion.POINTER_DOWN
import com.librestatic.ezdisplay.session.TopEdgeGesture.Companion.UP
import org.junit.Assert.assertEquals
import org.junit.Test

class TopEdgeGestureTest {
    // Zone 50 px, travel 100 px, slop 10 px.
    private fun gesture() = TopEdgeGesture(50f, 100f, 10f)

    private fun TopEdgeGesture.ev(a: Int, x: Float, y: Float, t: Long, n: Int = 1, stylus: Boolean = false) =
        onEvent(a, n, x, y, t, stylus)

    @Test fun acceptsADownwardSwipeFromTheTopEdge() {
        val g = gesture()
        assertEquals(EdgeResult.Hold, g.ev(DOWN, 500f, 10f, 0))
        assertEquals(EdgeResult.Hold, g.ev(MOVE, 505f, 60f, 100))
        assertEquals(EdgeResult.Gesture, g.ev(MOVE, 510f, 120f, 200))
        assertEquals(EdgeResult.Swallow, g.ev(MOVE, 510f, 300f, 250))
        assertEquals(EdgeResult.Swallow, g.ev(UP, 510f, 300f, 300))
        assertEquals(EdgeResult.Pass, g.ev(MOVE, 0f, 0f, 400)) // idle again
    }

    @Test fun rejectsAStylus() {
        assertEquals(EdgeResult.Pass, gesture().ev(DOWN, 500f, 10f, 0, stylus = true))
    }

    @Test fun rejectsAStartBelowTheZone() {
        val g = gesture()
        assertEquals(EdgeResult.Pass, g.ev(DOWN, 500f, 80f, 0))
        assertEquals(EdgeResult.Pass, g.ev(MOVE, 500f, 300f, 100))
        assertEquals(EdgeResult.Pass, g.ev(UP, 500f, 300f, 150))
    }

    @Test fun rejectsTwoPointers() {
        val g = gesture()
        assertEquals(EdgeResult.Pass, g.ev(DOWN, 500f, 10f, 0, n = 2))
        val h = gesture()
        h.ev(DOWN, 500f, 10f, 0)
        assertEquals(EdgeResult.Release, h.ev(POINTER_DOWN, 600f, 12f, 50, n = 2))
        assertEquals(EdgeResult.Pass, h.ev(MOVE, 600f, 300f, 100, n = 2))
    }

    @Test fun releasesWhenTheTravelIsTooSlow() {
        val g = gesture()
        g.ev(DOWN, 500f, 10f, 0)
        assertEquals(EdgeResult.Release, g.ev(MOVE, 500f, 120f, 450))
    }

    @Test fun timerReleasesAHeldGestureThatWentQuiet() {
        val g = gesture()
        g.ev(DOWN, 500f, 10f, 0)
        assertEquals(EdgeResult.Pass, g.checkTimeout(399))
        assertEquals(EdgeResult.Release, g.checkTimeout(400))
        assertEquals(EdgeResult.Pass, g.ev(MOVE, 500f, 400f, 500))
    }

    @Test fun releasesWhenMovingSideways() {
        val g = gesture()
        g.ev(DOWN, 500f, 10f, 0)
        assertEquals(EdgeResult.Release, g.ev(MOVE, 540f, 20f, 50))
        assertEquals(EdgeResult.Pass, g.ev(MOVE, 600f, 20f, 80))
    }

    @Test fun releasesWhenTheFingerLifts() {
        val g = gesture()
        g.ev(DOWN, 500f, 10f, 0)
        assertEquals(EdgeResult.Release, g.ev(UP, 500f, 10f, 50))
        assertEquals(EdgeResult.Hold, g.ev(DOWN, 500f, 10f, 100)) // ready for the next one
    }

    @Test fun releasesOnCancel() {
        val g = gesture()
        g.ev(DOWN, 500f, 10f, 0)
        assertEquals(EdgeResult.Release, g.ev(CANCEL, 500f, 10f, 50))
    }

    @Test fun rejectsADiagonalSwipe() {
        val g = gesture()
        g.ev(DOWN, 500f, 10f, 0)
        // dx grows past dy before the travel is reached.
        assertEquals(EdgeResult.Release, g.ev(MOVE, 580f, 50f, 50))
    }
}
