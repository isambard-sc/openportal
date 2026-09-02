// SPDX-FileCopyrightText: © 2026 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

package org.openportal;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertNotEquals;

import org.junit.jupiter.api.DisplayName;
import org.junit.jupiter.api.Test;

/**
 * The signing, checked against vectors rather than against opinion.
 *
 * <p>Every expected signature below was produced by an independent
 * implementation and then accepted by a running bridge, so a change that breaks
 * one of these breaks authentication - not a test's idea of it.
 */
class BridgeAuthTest {

    /** An obviously fake key: the vectors have to be reproducible. */
    private static final byte[] KEY =
            BridgeAuth.unhex("00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff");

    private static final String DATE = "Mon, 03 Aug 2026 12:00:00 GMT";

    @Test
    @DisplayName("the canonical string is seven length-prefixed fields")
    void canonicalShape() {
        assertEquals(
                "17:openportal-sig-v2\n"
                        + "3:get\n"
                        + "16:application/json\n"
                        + "29:Mon, 03 Aug 2026 12:00:00 GMT\n"
                        + "10:get_portal\n"
                        + "0:\n"
                        + "0:",
                BridgeAuth.canonicalString("get", DATE, "get_portal", "", ""));
    }

    @Test
    @DisplayName("an absent body and nonce are empty fields, not missing lines")
    void emptyFieldsArePresent() {
        String canonical = BridgeAuth.canonicalString("get", DATE, "health", "", null);

        assertEquals(7, canonical.split("\n", -1).length);
        assertEquals(true, canonical.endsWith("0:\n0:"));
    }

    @Test
    @DisplayName("the length prefix counts bytes, not characters")
    void lengthPrefixIsBytes() {
        // "café ☕" is six characters, nine bytes: e-acute is two and the emoji
        // three. A prefix from String.length() would be short, and the bridge
        // would reject only the requests that happen to carry an accent.
        String canonical = BridgeAuth.canonicalString("post", DATE, "run", "café ☕", "");

        assertEquals(true, canonical.contains("\n9:café ☕\n"));
    }

    @Test
    @DisplayName("the JSON encoding escapes control characters and leaves UTF-8 alone")
    void jsonEncoding() {
        assertEquals("\"a\\nb\"", BridgeAuth.jsonEncode("a\nb"));
        assertEquals("\"say \\\"hi\\\"\"", BridgeAuth.jsonEncode("say \"hi\""));
        assertEquals("\"back\\\\slash\"", BridgeAuth.jsonEncode("back\\slash"));

        // The negative rule, and the one that bites: non-ASCII is not escaped.
        assertEquals("\"café ☕\"", BridgeAuth.jsonEncode("café ☕"));
    }

    @Test
    @DisplayName("a GET with no nonce signs to the known vector")
    void getWithoutNonce() {
        assertEquals(
                "OpenPortal 61add4808751b10b317cc07a7f0c38084f9d6d2939eac218fd5dfce13005c829",
                BridgeAuth.authorization(KEY, "get", DATE, "get_portal", "", ""));
    }

    @Test
    @DisplayName("a GET with a nonce signs to the known vector")
    void getWithNonce() {
        assertEquals(
                "OpenPortal 18b4124a56fcecb449497af3335ae80d3f531aaf2f133dfd197bbb89c9008bba",
                BridgeAuth.authorization(KEY, "get", DATE, "get_portal", "", "unique-nonce-abc123"));
    }

    @Test
    @DisplayName("a POST with a body signs to the known vector")
    void postWithBody() {
        assertEquals(
                "OpenPortal bfdb4301644c61a062d9371a738a9eacbcf2d9602b210e0f95589f522446f2f5",
                BridgeAuth.authorization(
                        KEY,
                        "post",
                        DATE,
                        "run",
                        "{\"command\":\"waldur.provider get_offerings\"}",
                        "unique-nonce-abc123"));
    }

    @Test
    @DisplayName("a POST with a non-ASCII body signs to the known vector")
    void postWithUtf8Body() {
        assertEquals(
                "OpenPortal 2bd007f430d08cfa73dc406850f399d510568010e1acf8f814335fbb61ee4a4f",
                BridgeAuth.authorization(KEY, "post", DATE, "run", "{\"note\":\"café ☕\"}", "n"));
    }

    @Test
    @DisplayName("the nonce is authenticated, unlike in version 1")
    void nonceChangesTheSignature() {
        // The whole reason version 2 exists: in version 1 the presence of a
        // nonce was not distinguishable from a body ending in one.
        assertNotEquals(
                BridgeAuth.authorization(KEY, "post", DATE, "run", "{}", "a"),
                BridgeAuth.authorization(KEY, "post", DATE, "run", "{}", "b"));
    }

    @Test
    @DisplayName("the tag is 32 bytes, so 64 hex characters")
    void tagLength() {
        String signature = BridgeAuth.authorization(KEY, "get", DATE, "health", "", "");

        assertEquals(64, signature.substring("OpenPortal ".length()).length());
    }
}
