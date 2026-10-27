package com.librestatic.ezdisplay.screensaver

import kotlin.math.PI
import kotlin.math.cos
import kotlin.math.floor
import kotlin.math.hypot
import kotlin.math.sin
import kotlin.random.Random

/** How the screensaver shapes travel: across the screen and out again, or bouncing off the edges. */
enum class ScreensaverMotionMode(val key: String) {
    Drift("drift"),
    Bounce("bounce"),
    ;

    companion object {
        /** The mode stored under [key]; anything unknown is [Drift]. */
        fun fromKey(key: String?): ScreensaverMotionMode = entries.firstOrNull { it.key == key } ?: Drift
    }
}

/**
 * Pure motion model of the screensaver shapes (no Compose or Android types). Positions are pixel centres in a
 * field of [resize]d width and height. Each [Body] is treated as a circle of its [Body.radius].
 *
 * - [ScreensaverMotionMode.Bounce]: shapes start inside the field, move in straight lines and reflect
 *   when the circle touches an edge, so they never leave it.
 * - [ScreensaverMotionMode.Drift]: shapes spawn just outside a random edge, head for a random point in the middle
 *   of the field, cross it and, once fully outside again, respawn at a new edge position with a new heading.
 *   Some start already on screen so the field is not empty at first.
 *
 * Speed is [speedDp] dp per second for an average shape; each shape moves a little faster or slower by chance
 * and by depth. [density] is pixels per dp. The same [seed] always gives the same field and paths.
 */
class ScreensaverMotion(
    val mode: ScreensaverMotionMode,
    val count: Int,
    seed: Int,
    private val density: Float = 1f,
    speedDp: Float = DEFAULT_SPEED_DP,
) {
    /** One shape. Angles are in degrees, [spin] in degrees per second. */
    class Body internal constructor(
        val depth: Float,
        val radius: Float,
        val spin: Float,
        /** Random picks the renderer maps onto its own shape and colour lists with a modulo. */
        val shapeIndex: Int,
        val colorIndex: Int,
        private val speedFactor: Float,
    ) {
        var x = 0f
            internal set
        var y = 0f
            internal set
        var angle = 0f
            internal set
        internal var dirX = 1f
        internal var dirY = 0f

        /** True once a drifting shape has been (at least partly) on screen. */
        internal var entered = false
        internal var speedPx = 0f

        /** Current velocity in px per second. */
        val vx: Float get() = dirX * speedPx * speedFactor
        val vy: Float get() = dirY * speedPx * speedFactor

        internal fun factor(): Float = speedFactor
    }

    private val random = Random(seed)
    val bodies: List<Body> = List(count) {
        val depth = 0.4f + random.nextFloat() * 0.6f
        Body(
            depth = depth,
            radius = (MIN_SIZE_DP + SIZE_RANGE_DP * depth) / 2f * density,
            spin = (6f + random.nextFloat() * 14f) * if (random.nextBoolean()) 1f else -1f,
            shapeIndex = random.nextInt(1000),
            colorIndex = random.nextInt(1000),
            // Within about +-30% by chance, and nearer (bigger) shapes a bit faster than far ones.
            speedFactor = (0.7f + random.nextFloat() * 0.6f) * (0.8f + 0.4f * (depth - 0.4f) / 0.6f),
        )
    }

    private var width = 0f
    private var height = 0f

    /** Average speed in dp per second; changing it applies at once. */
    var speedDp: Float = speedDp
        set(value) {
            field = value
            applySpeed()
        }

    init {
        applySpeed()
    }

    private fun applySpeed() {
        val px = speedDp * density
        bodies.forEach { it.speedPx = px }
    }

    /** Sets the field size in px. The first call places the shapes; later ones clamp or respawn them. */
    fun resize(newWidth: Float, newHeight: Float) {
        if (newWidth <= 0f || newHeight <= 0f) return
        val first = width <= 0f
        if (!first && newWidth == width && newHeight == height) return
        width = newWidth
        height = newHeight
        bodies.forEachIndexed { index, b ->
            when {
                first -> place(b, staggered = mode == ScreensaverMotionMode.Bounce || index % 2 == 0)
                mode == ScreensaverMotionMode.Bounce -> {
                    b.x = clampAxis(b.x, b.radius, width)
                    b.y = clampAxis(b.y, b.radius, height)
                }
                intersects(b) -> b.entered = true
                else -> place(b, staggered = false)
            }
        }
    }

    /** Moves every shape forward by [dtSeconds] (any size; big steps fold bounces and respawn at most once). */
    fun step(dtSeconds: Float) {
        if (width <= 0f || dtSeconds <= 0f) return
        for (b in bodies) {
            val dist = b.speedPx * b.factor() * dtSeconds
            b.angle = (b.angle + b.spin * dtSeconds) % 360f
            when (mode) {
                ScreensaverMotionMode.Bounce -> bounce(b, dist)
                ScreensaverMotionMode.Drift -> {
                    b.x += b.dirX * dist
                    b.y += b.dirY * dist
                    if (intersects(b)) {
                        b.entered = true
                    } else if (b.entered) {
                        place(b, staggered = false)
                    }
                }
            }
        }
    }

    private fun bounce(b: Body, dist: Float) {
        val (nx, flipX) = fold(b.x + b.dirX * dist, b.radius, width)
        val (ny, flipY) = fold(b.y + b.dirY * dist, b.radius, height)
        b.x = nx
        b.y = ny
        if (flipX) b.dirX = -b.dirX
        if (flipY) b.dirY = -b.dirY
    }

    /**
     * Reflects an unfolded position [p] into [r, size - r]. The flag says whether the direction is reversed
     * afterwards (an odd number of reflections).
     */
    private fun fold(p: Float, r: Float, size: Float): Pair<Float, Boolean> {
        val span = size - 2f * r
        if (span <= 0f) return size / 2f to false
        val u = p - r
        val m = u - floor(u / (2f * span)) * 2f * span
        return if (m <= span) (r + m) to false else (r + 2f * span - m) to true
    }

    private fun clampAxis(p: Float, r: Float, size: Float): Float =
        if (size <= 2f * r) size / 2f else p.coerceIn(r, size - r)

    private fun intersects(b: Body): Boolean =
        b.x + b.radius > 0f && b.x - b.radius < width && b.y + b.radius > 0f && b.y - b.radius < height

    /** Puts [b] at a start position with a heading; [staggered] places it somewhere on screen, else off-screen. */
    private fun place(b: Body, staggered: Boolean) {
        when {
            mode == ScreensaverMotionMode.Bounce -> {
                b.x = randomInside(b.radius, width)
                b.y = randomInside(b.radius, height)
                // Keep clear of the axes so the path does not run along an edge.
                val quadrant = random.nextInt(4)
                val angle = (quadrant * 90f + 15f + random.nextFloat() * 60f) * PI.toFloat() / 180f
                b.dirX = cos(angle)
                b.dirY = sin(angle)
                b.entered = true
            }
            staggered -> {
                b.x = width * random.nextFloat()
                b.y = height * random.nextFloat()
                aimAtInnerPoint(b)
                b.entered = true
            }
            else -> {
                val t = random.nextFloat()
                when (random.nextInt(4)) {
                    0 -> { b.x = -b.radius; b.y = height * t }
                    1 -> { b.x = width + b.radius; b.y = height * t }
                    2 -> { b.x = width * t; b.y = -b.radius }
                    else -> { b.x = width * t; b.y = height + b.radius }
                }
                aimAtInnerPoint(b)
                b.entered = false
            }
        }
    }

    private fun randomInside(r: Float, size: Float): Float =
        if (size <= 2f * r) size / 2f else r + random.nextFloat() * (size - 2f * r)

    /** Heads for a random point in the middle 60% of the field, so a drifting shape always crosses it. */
    private fun aimAtInnerPoint(b: Body) {
        val tx = width * (0.2f + 0.6f * random.nextFloat())
        val ty = height * (0.2f + 0.6f * random.nextFloat())
        var dx = tx - b.x
        var dy = ty - b.y
        var len = hypot(dx, dy)
        if (len < 1f) {
            // Spawned on the target: any heading will do.
            val a = random.nextFloat() * 2f * PI.toFloat()
            dx = cos(a)
            dy = sin(a)
            len = 1f
        }
        b.dirX = dx / len
        b.dirY = dy / len
    }

    companion object {
        const val MIN_SIZE_DP = 40f
        const val SIZE_RANGE_DP = 120f
        const val DEFAULT_SPEED_DP = 40f
    }
}
