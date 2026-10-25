package com.librestatic.ezdisplay.wifi

import androidx.test.ext.junit.runners.AndroidJUnit4
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import org.junit.runner.RunWith

@RunWith(AndroidJUnit4::class)
class WifiModelTest {
    private val fpA = "3f2a91c077de0b14" + "00".repeat(24)
    private val fpB = "aaaabbbbccccdddd" + "11".repeat(24)

    @Test fun pinsAreSixDigits() {
        assertTrue(isValidPin("123456"))
        assertTrue(isValidPin("000000"))
        listOf("", "12345", "1234567", "12345a", " 12345", "١٢٣٤٥٦").forEach { assertFalse(it, isValidPin(it)) }
    }

    @Test fun shortFingerprintGroupsTheFirstSixteenDigits() {
        assertEquals("3f2a-91c0-77de-0b14", shortFingerprint(fpA.uppercase()))
        assertTrue(fingerprintMatches(fpA, "3f2a-91c0-77de-0b14"))
        assertTrue(fingerprintMatches(fpA.uppercase(), "3F2A-91C0-77DE-0B14"))
        assertFalse(fingerprintMatches(fpA, "3f2a-91c0-77de-0b15"))
        // Too short a prefix identifies nothing.
        assertFalse(fingerprintMatches(fpA, ""))
        assertFalse(fingerprintMatches(fpA, "3f2a"))
    }

    @Test fun parsesAPairingLink() {
        val link = parsePairUri("ezd://pair?a=192.168.1.5:27183&fp=$fpA&pin=123456&n=Studio%20PC%20%C3%A9")
        assertNotNull(link)
        assertEquals("192.168.1.5:27183", link!!.addr)
        assertEquals(fpA, link.fingerprint)
        assertEquals("123456", link.pin)
        assertEquals("Studio PC é", link.name)
        assertEquals("3f2a-91c0-77de-0b14", link.shortFp)
    }

    @Test fun aPairingLinkSurvivesTheRoundTripToTheNativeSide() {
        val link = PairLink("[fe80::1]:27183", fpA, "000123", "A&B = C+D %é")
        assertEquals(link, parsePairUri(link.toUri()))
        // The host decodes %XX only, so no raw spaces or plus signs.
        assertFalse(link.toUri().contains(' '))
        assertEquals("A%26B%20%3D%20C%2BD%20%25%C3%A9", percentEncode(link.name))
    }

    @Test fun rejectsBadPairingLinks() {
        val ok = "ezd://pair?a=10.0.0.2:27183&fp=$fpA&pin=123456&n=x"
        assertNotNull(parsePairUri(ok))
        assertNotNull(parsePairUri("  $ok  "))
        listOf(
            null, "", "https://example.com", "ezd://pair", "ezd://pair?",
            "ezd://pair?a=10.0.0.2:27183&fp=$fpA&pin=12345",
            "ezd://pair?a=10.0.0.2:27183&fp=$fpA&pin=abcdef",
            "ezd://pair?a=10.0.0.2:27183&fp=abcd&pin=123456",
            "ezd://pair?a=10.0.0.2:27183&pin=123456",
            "ezd://pair?a=not-an-address&fp=$fpA&pin=123456",
            "ezd://pair?a=300.0.0.1:27183&fp=$fpA&pin=123456",
            "ezd://pair?fp=$fpA&pin=123456",
            "ezd://pair?a=10.0.0.2:27183&fp=$fpA&pin=123456&n=%zz",
            "ezd://pair?a=10.0.0.2:27183&fp=$fpA&pin=123456&broken",
        ).forEach { assertNull(it, parsePairUri(it)) }
    }

    @Test fun aLinkWithoutANameIsStillValid() {
        assertEquals("", parsePairUri("ezd://pair?a=10.0.0.2:27183&fp=$fpA&pin=123456")!!.name)
    }

    @Test fun manualAddressesGetTheDefaultPort() {
        assertEquals("192.168.1.5:27183", parseManualAddress("192.168.1.5"))
        assertEquals("192.168.1.5:4000", parseManualAddress(" 192.168.1.5:4000 "))
        assertEquals("[fe80::1]:27183", parseManualAddress("fe80::1"))
        assertEquals("[2001:db8::2]:5000", parseManualAddress("[2001:db8::2]:5000"))
        assertEquals("[2001:db8::2]:27183", parseManualAddress("[2001:db8::2]"))
        listOf("", "   ", "desk", "1.2.3", "300.1.1.1", "1.2.3.4:0", "1.2.3.4:70000", "1.2.3.4:x", "1.2.3.4:", "[::1", "::zz")
            .forEach { assertNull(it, parseManualAddress(it)) }
    }

    @Test fun parsesHostsJson() {
        val json = """[{"name":"desk","fingerprint":"$fpA","shortFp":"3f2a-91c0-77de-0b14","addr":"10.0.0.2:27183"},
            {"name":"laptop","fingerprint":"$fpB","shortFp":"aaaa-bbbb-cccc-dddd"}]"""
        val hosts = parseHostsJson(json)
        assertEquals(2, hosts.size)
        assertEquals(WifiHost("desk", fpA, "3f2a-91c0-77de-0b14", "10.0.0.2:27183"), hosts[0])
        assertNull(hosts[1].addr)
        assertEquals(emptyList<WifiHost>(), parseHostsJson("[]"))
        assertEquals(emptyList<WifiHost>(), parseHostsJson("nope"))
    }

    @Test fun parsesPairResults() {
        val ok = parsePairResultJson(
            """{"ok":true,"host":{"name":"desk","fingerprint":"$fpA","shortFp":"3f2a-91c0-77de-0b14"}}""",
        )
        assertTrue(ok.ok)
        assertEquals("desk", ok.host!!.name)
        val bad = parsePairResultJson("""{"ok":false,"error":"wrong_pin","message":"refused"}""")
        assertFalse(bad.ok)
        assertEquals("wrong_pin", bad.error)
        assertEquals("other", parsePairResultJson("garbage").error)
    }

    @Test fun choosesTheLiveHostThenTheLastOneThenAStoredAddress() {
        val a = WifiHost("a", fpA, shortFingerprint(fpA), "10.0.0.2:27183")
        val b = WifiHost("b", fpB, shortFingerprint(fpB), "10.0.0.3:27183")
        val noAddr = WifiHost("c", "ff".repeat(32), "ffff-ffff-ffff-ffff", null)
        val seenB = DiscoveredHost("b-svc", "b", "aaaa-bbbb-cccc-dddd", "192.168.1.9:27183")

        // Discovery beats everything and brings the fresh address.
        assertEquals(WifiChoice(b, "192.168.1.9:27183"), chooseWifiTarget(listOf(a, b), listOf(seenB), fpA))
        // Nothing discovered: the last used host with its stored address.
        assertEquals(WifiChoice(b, "10.0.0.3:27183"), chooseWifiTarget(listOf(a, b), emptyList(), fpB))
        assertEquals(WifiChoice(a, "10.0.0.2:27183"), chooseWifiTarget(listOf(a, b), emptyList(), null))
        // A host with no address is skipped; none at all means no Wi-Fi attempt.
        assertEquals(WifiChoice(a, "10.0.0.2:27183"), chooseWifiTarget(listOf(noAddr, a), emptyList(), noAddr.fingerprint))
        assertNull(chooseWifiTarget(listOf(noAddr), emptyList(), null))
        assertNull(chooseWifiTarget(emptyList(), listOf(seenB), null))
        // A discovered host nobody paired is never chosen.
        assertNull(chooseWifiTarget(listOf(noAddr), listOf(seenB), null))
    }
}
