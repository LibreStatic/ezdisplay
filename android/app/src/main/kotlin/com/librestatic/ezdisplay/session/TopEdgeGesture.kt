package com.librestatic.ezdisplay.session

import kotlin.math.abs

/** What [TopEdgeGesture] decided about the event just fed to it. */
enum class EdgeResult {
    /** Not a candidate: forward the event normally. */
    Pass,

    /** A candidate that is not decided yet: hold the event. */
    Hold,

    /** Not the gesture after all: replay the held events (this one included) and forward normally. */
    Release,

    /** The top-edge swipe was just recognised: drop the held events, swallow the rest, show the pill. */
    Gesture,

    /** The rest of a recognised gesture: drop the event. */
    Swallow,
}

/**
 * Detects "a finger puts down inside the top zone and drags down" without depending on MotionEvent, so it can be
 * tested with plain numbers. Units are pixels and milliseconds; the caller converts dp.
 *
 * While undecided the events are [EdgeResult.Hold]; the caller keeps copies and replays them on
 * [EdgeResult.Release]. A stylus, a second pointer, a start below [topZonePx], leaving sideways, lifting or
 * [TIMEOUT_MS] without enough travel all release.
 */
class TopEdgeGesture(
    var topZonePx: Float,
    private val travelPx: Float,
    private val slopPx: Float,
) {
    enum class Phase { Idle, Undecided, Swallowing, Passing }

    var phase = Phase.Idle
        private set
    private var downX = 0f
    private var downY = 0f
    private var downMs = 0L

    /** [action] is a [android.view.MotionEvent] action masked value: 0 down, 1 up, 2 move, 3 cancel, 5 pointer down, 6 pointer up. */
    fun onEvent(action: Int, pointerCount: Int, x: Float, y: Float, timeMs: Long, isStylus: Boolean): EdgeResult {
        when (action) {
            DOWN -> {
                if (!isStylus && pointerCount == 1 && y < topZonePx) {
                    phase = Phase.Undecided
                    downX = x
                    downY = y
                    downMs = timeMs
                    return EdgeResult.Hold
                }
                phase = Phase.Passing
                return EdgeResult.Pass
            }
        }
        return when (phase) {
            Phase.Idle -> EdgeResult.Pass
            Phase.Passing -> {
                if (action == UP || action == CANCEL) phase = Phase.Idle
                EdgeResult.Pass
            }
            Phase.Swallowing -> {
                if (action == UP || action == CANCEL) phase = Phase.Idle
                EdgeResult.Swallow
            }
            Phase.Undecided -> decide(action, pointerCount, x, y, timeMs)
        }
    }

    private fun decide(action: Int, pointerCount: Int, x: Float, y: Float, timeMs: Long): EdgeResult {
        if (action == CANCEL || action == UP) {
            phase = Phase.Idle
            return EdgeResult.Release
        }
        if (action == POINTER_DOWN || pointerCount > 1) return release()
        val dx = abs(x - downX)
        val dy = y - downY
        if (dy >= travelPx && timeMs - downMs <= TIMEOUT_MS && dx < dy) {
            phase = Phase.Swallowing
            return EdgeResult.Gesture
        }
        if (timeMs - downMs >= TIMEOUT_MS) return release()
        // Moved beyond the touch slop without heading down: it is a normal touch.
        if (dx > slopPx && dx >= dy || -dy > slopPx) return release()
        return EdgeResult.Hold
    }

    /** For a timer: releases an undecided gesture whose finger stopped sending events for [TIMEOUT_MS]. */
    fun checkTimeout(timeMs: Long): EdgeResult =
        if (phase == Phase.Undecided && timeMs - downMs >= TIMEOUT_MS) release() else EdgeResult.Pass

    private fun release(): EdgeResult {
        phase = Phase.Passing
        return EdgeResult.Release
    }

    /** Forget any gesture in progress (e.g. when input stops being forwarded). */
    fun reset() {
        phase = Phase.Idle
    }

    companion object {
        const val TIMEOUT_MS = 400L
        const val DOWN = 0
        const val UP = 1
        const val MOVE = 2
        const val CANCEL = 3
        const val POINTER_DOWN = 5
        const val POINTER_UP = 6
    }
}
