package com.librestatic.ezdisplay.usb

/** Manufacturer and model the PC announces when it switches the tablet into accessory mode. */
const val ACCESSORY_MANUFACTURER = "LibreStatic"
const val ACCESSORY_MODEL = "ezdisplay"

/** True for the USB accessory the ezdisplay PC host presents (see `res/xml/accessory_filter.xml`). */
fun isOurAccessory(manufacturer: String?, model: String?): Boolean =
    manufacturer == ACCESSORY_MANUFACTURER && model == ACCESSORY_MODEL

/** How the next connection attempt reaches the host. */
enum class Route { ACCESSORY, TCP, WIFI }

/**
 * Connection order: USB accessory, then the adb-reversed loopback port, then Wi-Fi.
 * An attached accessory always wins (the PC holds its session slot for it). Wi-Fi is only tried right
 * after a loopback attempt failed ([loopbackFailed]) and a paired host is available ([wifiAvailable]).
 */
fun chooseRoute(accessoryReady: Boolean, loopbackFailed: Boolean, wifiAvailable: Boolean): Route = when {
    accessoryReady -> Route.ACCESSORY
    loopbackFailed && wifiAvailable -> Route.WIFI
    else -> Route.TCP
}

/** What to do with the current session when an accessory shows up. */
fun shouldSwitchToAccessory(accessoryReady: Boolean, sessionActive: Boolean, sessionIsAccessory: Boolean): Boolean =
    accessoryReady && !(sessionActive && sessionIsAccessory)
