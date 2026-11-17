package com.librestatic.ezdisplay

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class HostLockTest {
    @Test fun onlyTheScreenLockedCodeMeansLocked() {
        assertTrue(HostLock.isLocked("screen_locked"))
        assertFalse(HostLock.isLocked(null))
        assertFalse(HostLock.isLocked("not_paired"))
    }

    @Test fun connectingTextsDoNotReplaceTheLockedMessage() {
        assertFalse(HostLock.showsProgress(true))
        assertTrue(HostLock.showsProgress(false))
    }

    @Test fun aNegotiatedStreamCountsOnlyOnceAFrameArrives() {
        assertFalse(HostLock.confirmsUnlock(0))
        assertTrue(HostLock.confirmsUnlock(1))
    }

    @Test fun retriesAreSlowAndFixed() {
        assertEquals(5000L, HostLock.RETRY_MS)
    }
}
