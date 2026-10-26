package com.librestatic.ezdisplay.settings

import androidx.compose.ui.semantics.SemanticsProperties
import androidx.compose.ui.test.assertIsEnabled
import androidx.compose.ui.test.assertIsNotEnabled
import androidx.compose.ui.test.assertTextContains
import androidx.compose.ui.test.assertTextEquals
import androidx.compose.ui.test.performTextClearance
import androidx.compose.ui.test.onNodeWithText
import androidx.compose.ui.test.performClick
import androidx.compose.ui.test.performScrollTo
import androidx.compose.ui.test.performTextInput
import com.librestatic.ezdisplay.R
import com.librestatic.ezdisplay.wifi.DiscoveredHost
import com.librestatic.ezdisplay.wifi.PairLink
import com.librestatic.ezdisplay.wifi.WifiHost
import com.librestatic.ezdisplay.wifi.isLocalAddress
import com.librestatic.ezdisplay.wifi.parsePairUri
import com.librestatic.ezdisplay.wifi.shortFingerprint
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import org.robolectric.annotation.Config

private val FP = "ab".repeat(32)
private val OTHER_FP = "cd".repeat(32)
private val PAIRED = WifiHost("Laptop", OTHER_FP, shortFingerprint(OTHER_FP), "10.0.0.7:27183")
private val SEEN = DiscoveredHost("desk-svc", "Studio PC", shortFingerprint(FP), "192.168.1.9:27183")

/** The Connection section on a phone: one scrolling column. */
@Config(sdk = [36], qualifiers = "w412dp-h915dp")
class SettingsConnectionPhoneTest : SettingsUiTestBase() {
    private fun openConnection(backend: FakeWifiBackend = FakeWifiBackend(listOf(PAIRED))) {
        launch(wifi = backend)
        rule.runOnUiThread { store.setDiscovered(listOf(SEEN)) }
        rule.waitForIdle()
        tag("connection_content").performScrollTo()
    }

    @Test fun showsTheSwitchWarningHostsAndFingerprint() {
        openConnection()
        dumpTree("phone-connection")
        rule.onNodeWithText(str(R.string.wifi_latency_warning)).performScrollTo().assertExists()
        tag("wifi_row").assertExists()
        tag("paired_$OTHER_FP").performScrollTo().assertExists()
        tag("discovered_desk-svc").performScrollTo().assertExists()
        tag("manual_row").performScrollTo().assertExists()
        tag("device_fp").performScrollTo().assertExists()
        rule.onNodeWithText("abcd-0123-4567-89ef").assertExists()
    }

    @Test fun theSwitchStartsOffAndSaves() {
        launch(wifi = FakeWifiBackend())
        tag("wifi_row").performScrollTo().performClick()
        assertTrue(prefs.getBoolean(SettingsStore.WIFI_ENABLED, false))
        assertTrue(store.state.value.wifiEnabled)
        tag("wifi_row").performClick()
        assertFalse(prefs.getBoolean(SettingsStore.WIFI_ENABLED, true))
    }

    @Test fun emptyListsSayWhatToDo() {
        launch(wifi = FakeWifiBackend())
        tag("paired_empty").performScrollTo().assertExists()
        tag("discovered_empty").performScrollTo().assertExists()
    }

    @Test fun pairedHostsAreNotOfferedAgain() {
        launch(wifi = FakeWifiBackend(listOf(WifiHost("Studio PC", FP, shortFingerprint(FP), null))))
        rule.runOnUiThread { store.setDiscovered(listOf(SEEN)) }
        rule.waitForIdle()
        tag("discovered_desk-svc").assertDoesNotExist()
        tag("discovered_empty").performScrollTo().assertExists()
    }

    @Test fun pairingAFoundHostAsksForThePin() {
        val backend = FakeWifiBackend(listOf(PAIRED), pin = "424242")
        openConnection(backend)
        tag("pair_desk-svc").performScrollTo().performClick()
        tag("pair_dialog").assertExists()
        rule.onNodeWithText(str(R.string.pair_dialog_title, "Studio PC")).assertExists()
        tag("pair_fp").assertTextContains(shortFingerprint(FP), substring = true)
        tag("pair_confirm").assertIsNotEnabled()

        // Only digits, at most six.
        tag("pin_field").performTextInput("12ab34567")
        tag("pin_field").assertTextContains("123456")
        tag("pair_confirm").assertIsEnabled()

        // A wrong PIN keeps the dialog open with the message and nothing is paired.
        tag("pair_confirm").performClick()
        rule.waitForIdle()
        assertEquals(listOf(Triple("192.168.1.9:27183", "123456", "test tablet")), backend.calls)
        tag("pair_error").assertTextContains(str(R.string.pair_error_wrong_pin))
        assertEquals(1, store.state.value.wifiHosts.size)
    }

    @Test fun aCorrectPinPairsClosesTheDialogAndEnablesWifi() {
        val backend = FakeWifiBackend(listOf(PAIRED), pin = "424242")
        openConnection(backend)
        assertFalse(store.state.value.wifiEnabled)
        tag("pair_desk-svc").performScrollTo().performClick()
        tag("pin_field").performTextInput("424242")
        tag("pair_confirm").performClick()
        rule.waitForIdle()
        tag("pair_dialog").assertDoesNotExist()
        assertNull(store.state.value.pairing)
        assertEquals(2, store.state.value.wifiHosts.size)
        assertTrue(store.state.value.wifiEnabled)
        assertTrue(prefs.getBoolean(SettingsStore.WIFI_ENABLED, false))
        // The paired host moved from "found" to "paired".
        tag("paired_$FP").performScrollTo().assertExists()
        tag("discovered_desk-svc").assertDoesNotExist()
    }

    @Test fun cancelClosesTheDialogWithoutPairing() {
        val backend = FakeWifiBackend()
        openConnection(backend)
        tag("pair_desk-svc").performScrollTo().performClick()
        tag("pair_cancel").performClick()
        tag("pair_dialog").assertDoesNotExist()
        assertTrue(backend.calls.isEmpty())
    }

    @Test fun forgetRemovesThePairedHost() {
        val backend = FakeWifiBackend(listOf(PAIRED))
        openConnection(backend)
        tag("forget_$OTHER_FP").performScrollTo().performClick()
        assertTrue(backend.paired.isEmpty())
        assertTrue(store.state.value.wifiHosts.isEmpty())
        tag("paired_empty").performScrollTo().assertExists()
    }

    @Test fun theManualPairButtonNeedsAValidAddress() {
        openConnection()
        tag("manual_pair").performScrollTo().assertIsNotEnabled()
        tag("manual_field").performTextInput("not an address")
        tag("manual_pair").assertIsNotEnabled()
    }

    @Test fun aManualAddressGetsTheDefaultPort() {
        val backend = FakeWifiBackend()
        openConnection(backend)
        tag("manual_field").performScrollTo().performTextInput("192.168.1.20")
        tag("manual_pair").assertIsEnabled().performClick()
        rule.onNodeWithText(str(R.string.pair_dialog_title, "192.168.1.20:27183")).assertExists()
        tag("pin_field").performTextInput("123456")
        tag("pair_confirm").performClick()
        rule.waitForIdle()
        assertEquals("192.168.1.20:27183", backend.calls.single().first)
    }

    @Test fun aScannedCodeAsksForConfirmationAndPairsWithoutAPin() {
        val backend = FakeWifiBackend()
        launch(wifi = backend)
        val link = PairLink("192.168.1.9:27183", FP, "424242", "Studio PC")
        rule.runOnUiThread { store.startPairing(link) }
        rule.waitForIdle()
        dumpTree("phone-scanned")
        tag("pair_dialog").assertExists()
        rule.onNodeWithText(str(R.string.pair_dialog_title, "Studio PC")).assertExists()
        tag("pair_fp").assertTextContains(shortFingerprint(FP), substring = true)
        tag("pin_field").assertDoesNotExist()
        tag("pair_addr").assertTextContains("192.168.1.9:27183", substring = true)
        tag("pair_pin").assertTextEquals("424242")
        rule.onNodeWithText(str(R.string.pair_dialog_pin_check)).assertExists()
        tag("pair_confirm").assertIsEnabled().performClick()
        rule.waitForIdle()
        val call = backend.calls.single()
        assertEquals(link.toUri(), call.second)
        tag("pair_dialog").assertDoesNotExist()
        assertEquals(1, store.state.value.wifiHosts.size)
    }

    @Test fun aManualPublicAddressShowsAWarning() {
        openConnection()
        tag("manual_field").performScrollTo().performTextInput("192.168.1.20")
        rule.onNodeWithText(str(R.string.wifi_manual_public_warning)).assertDoesNotExist()
        tag("manual_field").performTextClearance()
        tag("manual_field").performTextInput("203.0.113.9")
        rule.onNodeWithText(str(R.string.wifi_manual_public_warning)).assertExists()
        // The address is still allowed: the user typed it.
        tag("manual_pair").assertIsEnabled()
    }

    @Test fun aPublicAddressLinkIsRefusedWithAnError() {
        val link = parsePairUri("ezd://pair?a=203.0.113.9:27183&fp=$FP&pin=424242&n=Office%20PC")!!
        assertFalse(isLocalAddress(link.addr))
        assertEquals(R.string.pair_error_not_local, wifiErrorText("not_local_address"))
        assertTrue(str(R.string.pair_error_not_local).isNotEmpty())
    }

    @Test fun anUnreachableHostShowsItsMessage() {
        val backend = FakeWifiBackend(error = "unreachable")
        openConnection(backend)
        tag("pair_desk-svc").performScrollTo().performClick()
        tag("pin_field").performTextInput("000000")
        tag("pair_confirm").performClick()
        rule.waitForIdle()
        tag("pair_error").assertTextContains(str(R.string.pair_error_unreachable))
        // The dialog can be retried: the button is enabled again.
        tag("pair_confirm").assertIsEnabled()
    }

    @Test fun everyErrorCodeHasItsOwnMessage() {
        val codes = listOf(
            "wrong_pin", "pairing_closed", "too_many_attempts", "busy", "not_paired", "unreachable", "timeout",
            "fingerprint_mismatch", "invalid_pin", "invalid_uri", "invalid_address",
        )
        assertEquals(codes.size, codes.map { wifiErrorText(it) }.toSet().size)
        assertEquals(R.string.pair_error_other, wifiErrorText("something new"))
        assertEquals(R.string.pair_error_other, wifiErrorText(null))
        // The pairing view model never leaks a missing code.
        assertFalse(codes.contains("other"))
    }

    @Test fun pinValidationRunsBeforeAnythingIsSent() {
        val backend = FakeWifiBackend()
        launch(wifi = backend)
        rule.runOnUiThread { store.startPairing("x", "", "10.0.0.1:27183"); store.submitPairing("12") }
        rule.waitForIdle()
        assertEquals("invalid_pin", store.state.value.pairing?.error)
        assertTrue(backend.calls.isEmpty())
        val semantics = tag("pair_error").fetchSemanticsNode().config[SemanticsProperties.Text].joinToString { it.text }
        assertEquals(str(R.string.pair_error_invalid_pin), semantics)
    }
}

/** On a foldable the section is a list entry with the content as detail. */
@Config(sdk = [36], qualifiers = "w841dp-h701dp")
class SettingsConnectionListDetailTest : SettingsUiTestBase() {
    @Test fun theSectionSummarizesAndShowsTheContent() {
        launch(wifi = FakeWifiBackend(listOf(PAIRED)))
        tag("section_Connection").assertExists()
        rule.onNodeWithText(str(R.string.connection_summary_usb)).assertExists()
        tag("section_Connection").performClick()
        tag("connection_content").assertExists()
        tag("paired_$OTHER_FP").assertExists()
        tag("wifi_row").performClick()
        rule.onNodeWithText("Wi-Fi · 1 paired host").assertExists()
    }
}

/** On a tablet the page shows the switch and warning, and the hosts open in a dialog so the page never scrolls. */
@Config(sdk = [36], qualifiers = "w1691dp-h978dp-280dpi")
class SettingsConnectionWideTest : SettingsUiTestBase() {
    private fun scrollMax(): Float = tag("wide_page").fetchSemanticsNode().config[SemanticsProperties.VerticalScrollAxisRange].maxValue()

    @Test fun pageShowsSwitchWarningAndAManageRow() {
        launch(wifi = FakeWifiBackend(listOf(PAIRED)))
        dumpTree("wide-connection")
        tag("wifi_row").assertExists()
        tag("wifi_warning").assertExists()
        tag("connection_manage").assertExists()
        tag("paired_$OTHER_FP").assertDoesNotExist()
    }

    @Test fun theManageRowOpensTheHostsAndPairingWorks() {
        val backend = FakeWifiBackend(listOf(PAIRED), pin = "111111")
        launch(wifi = backend)
        rule.runOnUiThread { store.setDiscovered(listOf(SEEN)) }
        tag("connection_manage").performClick()
        tag("hosts_dialog").assertExists()
        tag("paired_$OTHER_FP").assertExists()
        tag("pair_desk-svc").performClick()
        tag("pin_field").performTextInput("111111")
        tag("pair_confirm").performClick()
        rule.waitForIdle()
        assertEquals(2, store.state.value.wifiHosts.size)
        tag("hosts_dialog_close").performClick()
        tag("hosts_dialog").assertDoesNotExist()
    }

    @Test fun pageDoesNotScrollWithManyHostsAndEveryProfile() {
        val many = (1..8).map { WifiHost("Host $it", "%02x".format(it).repeat(32), "0$it-0$it-0$it-0$it", "10.0.0.$it:27183") }
        launch(AirActions.On, FakeWifiBackend(many))
        rule.runOnUiThread {
            store.setDiscovered((1..5).map { DiscoveredHost("svc$it", "Found $it", "ee$it-ee$it-ee$it-ee$it", "10.1.0.$it:27183") })
            store.setWifiEnabled(true)
        }
        for (profile in listOf("raw", "desktop", "drawing", "graphics_tablet")) {
            rule.runOnUiThread { store.setProfile(profile) }
            rule.waitForIdle()
            assertEquals("page scrolls with profile $profile", 0f, scrollMax(), 0f)
        }
    }
}
