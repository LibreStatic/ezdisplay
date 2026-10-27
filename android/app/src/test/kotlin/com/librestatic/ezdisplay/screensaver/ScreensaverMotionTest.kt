package com.librestatic.ezdisplay.screensaver

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test
import kotlin.math.hypot

class ScreensaverMotionTest {
    private val w = 1000f
    private val h = 600f

    private fun motion(mode: ScreensaverMotionMode, seed: Int = 1, speedDp: Float = 40f) =
        ScreensaverMotion(mode, 6, seed, density = 1f, speedDp = speedDp).also { it.resize(w, h) }

    private fun inside(b: ScreensaverMotion.Body) =
        b.x - b.radius >= -0.01f && b.x + b.radius <= w + 0.01f && b.y - b.radius >= -0.01f && b.y + b.radius <= h + 0.01f

    private fun intersects(b: ScreensaverMotion.Body) =
        b.x + b.radius > 0f && b.x - b.radius < w && b.y + b.radius > 0f && b.y - b.radius < h

    @Test fun bounceKeepsEveryShapeInsideTheField() {
        val m = motion(ScreensaverMotionMode.Bounce, speedDp = 100f)
        assertTrue(m.bodies.all(::inside))
        repeat(20_000) {
            m.step(1 / 60f)
            assertTrue(m.bodies.all(::inside))
        }
    }

    @Test fun bounceKeepsShapesInsideWithHugeSteps() {
        val m = motion(ScreensaverMotionMode.Bounce, speedDp = 100f)
        repeat(500) {
            m.step(4f)
            assertTrue(m.bodies.all(::inside))
        }
    }

    @Test fun bounceReversesVelocityAtAnEdge() {
        val m = motion(ScreensaverMotionMode.Bounce, speedDp = 100f)
        var x = m.bodies.map { it.vx }
        var flips = 0
        repeat(5_000) {
            m.step(1 / 60f)
            val now = m.bodies.map { it.vx }
            flips += x.indices.count { now[it] * x[it] < 0f }
            x = now
        }
        assertTrue(flips > 0)
        // Speed stays the same through a bounce.
        val b = m.bodies.first()
        assertEquals(100f * (b.vx.let { hypot(it, b.vy) } / 100f), hypot(b.vx, b.vy), 0.001f)
    }

    @Test fun driftSpawnsOffScreenExceptStaggeredShapes() {
        val m = motion(ScreensaverMotionMode.Drift)
        val onScreen = m.bodies.count(::intersects)
        assertTrue(onScreen in 1 until m.bodies.size)
        assertEquals(m.bodies.size / 2, onScreen)
    }

    @Test fun driftShapesEnterLeaveAndRespawnOffScreen() {
        val m = motion(ScreensaverMotionMode.Drift, speedDp = 100f)
        val wasInside = BooleanArray(m.bodies.size)
        var left = 0
        var respawned = 0
        repeat(60 * 600) {
            m.step(1 / 60f)
            m.bodies.forEachIndexed { i, b ->
                val now = intersects(b)
                if (wasInside[i] && !now) left++
                if (!wasInside[i] && now) respawned++
                wasInside[i] = now
            }
        }
        // Every body crossed the view many times: entered, left, and came back from outside.
        assertTrue(left >= m.bodies.size * 5)
        assertTrue(respawned >= m.bodies.size * 5)
    }

    @Test fun driftRespawnIsFullyOutsideTheView() {
        val m = motion(ScreensaverMotionMode.Drift, speedDp = 100f)
        var prevEntered = m.bodies.map(::intersects)
        repeat(60 * 300) {
            m.step(1 / 60f)
            val now = m.bodies.map(::intersects)
            m.bodies.forEachIndexed { i, b ->
                if (!prevEntered[i] && now[i]) {
                    // Just appeared: must be a body that is arriving, not popping into the middle.
                    assertTrue(b.x < 0f + b.radius + 5f || b.x > w - b.radius - 5f || b.y < b.radius + 5f || b.y > h - b.radius - 5f)
                }
            }
            prevEntered = now
        }
    }

    @Test fun speedScalesDistancePerSecond() {
        fun travelled(speedDp: Float): Float {
            val m = motion(ScreensaverMotionMode.Drift, speedDp = speedDp)
            val b = m.bodies.first()
            val x0 = b.x
            val y0 = b.y
            m.step(0.01f)
            return hypot(b.x - x0, b.y - y0)
        }
        val slow = travelled(10f)
        assertTrue(slow > 0f)
        assertEquals(slow * 5f, travelled(50f), 0.001f)
    }

    @Test fun changingSpeedAppliesAtOnce() {
        val m = motion(ScreensaverMotionMode.Bounce, speedDp = 10f)
        val b = m.bodies.first()
        val slow = hypot(b.vx, b.vy)
        m.speedDp = 20f
        assertEquals(slow * 2f, hypot(b.vx, b.vy), 0.001f)
    }

    @Test fun sameSeedGivesSamePaths() {
        fun run(seed: Int): List<Float> {
            val m = motion(ScreensaverMotionMode.Drift, seed)
            repeat(1000) { m.step(1 / 60f) }
            return m.bodies.flatMap { listOf(it.x, it.y, it.angle) }
        }
        assertEquals(run(7), run(7))
        assertFalse(run(7) == run(8))
    }

    @Test fun resizeClampsBouncingShapesAndRespawnsOutsideOnes() {
        val bounce = motion(ScreensaverMotionMode.Bounce)
        bounce.resize(600f, 1000f)
        assertTrue(bounce.bodies.all { it.x - it.radius >= -0.01f && it.x + it.radius <= 600.01f && it.y + it.radius <= 1000.01f })
        val drift = motion(ScreensaverMotionMode.Drift)
        drift.resize(300f, 300f)
        repeat(60 * 120) { drift.step(1 / 60f) }
        assertTrue(drift.bodies.any(::intersectsSmall))
    }

    private fun intersectsSmall(b: ScreensaverMotion.Body) =
        b.x + b.radius > 0f && b.x - b.radius < 300f && b.y + b.radius > 0f && b.y - b.radius < 300f

    @Test fun unknownModeKeyIsDrift() {
        assertEquals(ScreensaverMotionMode.Drift, ScreensaverMotionMode.fromKey("spiral"))
        assertEquals(ScreensaverMotionMode.Drift, ScreensaverMotionMode.fromKey(null))
        assertEquals(ScreensaverMotionMode.Bounce, ScreensaverMotionMode.fromKey("bounce"))
    }
}
