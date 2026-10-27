package com.librestatic.ezdisplay

import android.media.AudioManager
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

class VolumeSyncTest {
    private class FakeMusic(override val max: Int = 15, start: Int = 7) : MusicVolume {
        override var index = start
        override var muted = false
        var sets = 0
        var muteCalls = 0
    }

    private class Rig(val music: FakeMusic = FakeMusic()) {
        val reports = mutableListOf<Pair<Float, Boolean>>()
        var host = floatArrayOf(0f, 0f, 0f)
        val sync = VolumeSync(music, { v, m -> reports.add(v to m) }, { host })
    }

    @Test fun mapsStepsAndPerceptualVolume() {
        assertEquals(0f, volumeOfIndex(0, 15), 0f)
        assertEquals(1f, volumeOfIndex(15, 15), 0f)
        assertEquals(0.5f, volumeOfIndex(5, 10), 1e-6f)
        assertEquals(8, indexOfVolume(0.5f, 15))
        assertEquals(15, indexOfVolume(1.5f, 15))
        assertEquals(0, indexOfVolume(-1f, 15))
        for (i in 0..15) assertEquals(i, indexOfVolume(volumeOfIndex(i, 15), 15))
    }

    @Test fun beginReportsTheCurrentVolume() {
        val r = Rig()
        r.sync.begin()
        assertEquals(listOf(7 / 15f to false), r.reports)
        // Nothing changed: no new report.
        r.sync.onLocalChange()
        assertEquals(1, r.reports.size)
    }

    @Test fun reportsLocalChangesOnce() {
        val r = Rig()
        r.sync.begin()
        r.music.index = 9
        r.sync.onLocalChange()
        r.sync.onLocalChange()
        r.music.muted = true
        r.sync.onLocalChange()
        assertEquals(listOf(7 / 15f to false, 9 / 15f to false, 9 / 15f to true), r.reports)
    }

    @Test fun appliesTheHostVolumeWithoutEchoingIt() {
        val r = Rig()
        r.sync.begin()
        r.host = floatArrayOf(1f, 0.5f, 0f) // 0.5 * 15 = 7.5, rounds to 8
        r.sync.pollHost()
        assertEquals(8, r.music.index)
        // The settings notification that follows is not sent back, even though 8/15 != 0.5.
        r.sync.onLocalChange()
        assertEquals(1, r.reports.size)
        // The same message again changes nothing.
        r.music.index = 3
        r.sync.pollHost()
        assertEquals(3, r.music.index)
        r.sync.onLocalChange()
        assertEquals(listOf(7 / 15f to false, 3 / 15f to false), r.reports)
    }

    @Test fun appliesTheHostMuteLast() {
        val r = Rig()
        r.sync.begin()
        r.host = floatArrayOf(1f, 0.2f, 1f)
        r.sync.pollHost()
        assertEquals(3, r.music.index)
        assertTrue(r.music.muted)
        r.sync.onLocalChange()
        assertEquals(1, r.reports.size)
        r.host = floatArrayOf(2f, 0.2f, 0f)
        r.sync.pollHost()
        assertEquals(false, r.music.muted)
        r.sync.onLocalChange()
        assertEquals(1, r.reports.size)
    }

    @Test fun ignoresMissingOrShortHostData() {
        val r = Rig()
        r.sync.begin()
        r.sync.pollHost() // seq 0: nothing yet
        r.host = floatArrayOf(1f)
        r.sync.pollHost()
        assertEquals(7, r.music.index)
    }

    @Test fun aNewSessionStartsFromSequenceZero() {
        val r = Rig()
        r.sync.begin()
        r.host = floatArrayOf(5f, 1f, 0f)
        r.sync.pollHost()
        assertEquals(15, r.music.index)
        r.sync.begin() // the next session numbers its messages from 1 again
        r.host = floatArrayOf(1f, 0f, 0f)
        r.sync.pollHost()
        assertEquals(0, r.music.index)
    }

    @Test fun volumeKeysStepAndReport() {
        val r = Rig()
        r.sync.begin()
        r.sync.onKey(AudioManager.ADJUST_RAISE)
        r.sync.onKey(AudioManager.ADJUST_TOGGLE_MUTE)
        r.music.index = 15
        r.sync.onKey(AudioManager.ADJUST_RAISE) // already at the top: clamped
        assertEquals(listOf(7 / 15f to false, 8 / 15f to false, 8 / 15f to true, 1f to true), r.reports)
    }
}
