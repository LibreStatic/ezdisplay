package com.librestatic.ezdisplay.screensaver

/**
 * Measures how much of the screen changed, from small ARGB samples of it. Pure Kotlin.
 *
 * The reference is the screen at the last content activity, not the previous sample: a slow transition adds up
 * until it crosses the wake threshold, while an icon that blinks on and off never does.
 */
class ScreenChangeDetector {
    private var reference: IntArray? = null

    /**
     * Fraction (0..1) of the cells whose R, G or B differs from the reference by more than [TOLERANCE], or null
     * when there was no reference yet (then [sample] becomes the reference). Does not move the reference.
     */
    fun changedFraction(sample: IntArray): Float? {
        val ref = reference
        if (ref == null || ref.size != sample.size) {
            reference = sample.copyOf()
            return null
        }
        if (sample.isEmpty()) return 0f
        var changed = 0
        for (i in sample.indices) {
            val a = ref[i]
            val b = sample[i]
            if (channelDiff(a, b, 16) > TOLERANCE || channelDiff(a, b, 8) > TOLERANCE || channelDiff(a, b, 0) > TOLERANCE) {
                changed++
            }
        }
        return changed.toFloat() / sample.size
    }

    /** Makes [sample] the new reference. */
    fun accept(sample: IntArray) {
        val ref = reference
        if (ref != null && ref.size == sample.size) sample.copyInto(ref) else reference = sample.copyOf()
    }

    /** Forgets the reference; the next sample becomes the new one. */
    fun reset() {
        reference = null
    }

    private fun channelDiff(a: Int, b: Int, shift: Int): Int = Math.abs(((a shr shift) and 0xFF) - ((b shr shift) and 0xFF))

    companion object {
        /** Per-channel difference (of 255) that is ignored as codec noise or dithering. */
        const val TOLERANCE = 24
    }
}
