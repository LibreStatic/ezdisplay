package com.librestatic.ezdisplay.usb

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class AccessoryModelTest {
    @Test fun matchesOnlyOurManufacturerAndModel() {
        assertTrue(isOurAccessory("LibreStatic", "ezdisplay"))
        assertFalse(isOurAccessory("LibreStatic", "other"))
        assertFalse(isOurAccessory("Other", "ezdisplay"))
        assertFalse(isOurAccessory(null, "ezdisplay"))
        assertFalse(isOurAccessory("LibreStatic", null))
        assertFalse(isOurAccessory("librestatic", "ezdisplay"))
    }

    @Test fun accessoryBeatsEverything() {
        assertEquals(Route.ACCESSORY, chooseRoute(accessoryReady = true, loopbackFailed = true, wifiAvailable = true))
        assertEquals(Route.ACCESSORY, chooseRoute(accessoryReady = true, loopbackFailed = false, wifiAvailable = false))
    }

    @Test fun loopbackIsTriedBeforeWifi() {
        assertEquals(Route.TCP, chooseRoute(accessoryReady = false, loopbackFailed = false, wifiAvailable = true))
        assertEquals(Route.WIFI, chooseRoute(accessoryReady = false, loopbackFailed = true, wifiAvailable = true))
        assertEquals(Route.TCP, chooseRoute(accessoryReady = false, loopbackFailed = true, wifiAvailable = false))
    }

    @Test fun switchesToAccessoryUnlessAlreadyOnIt() {
        assertTrue(shouldSwitchToAccessory(true, sessionActive = false, sessionIsAccessory = false))
        assertTrue(shouldSwitchToAccessory(true, sessionActive = true, sessionIsAccessory = false))
        assertFalse(shouldSwitchToAccessory(true, sessionActive = true, sessionIsAccessory = true))
        assertFalse(shouldSwitchToAccessory(false, sessionActive = true, sessionIsAccessory = false))
    }
}
