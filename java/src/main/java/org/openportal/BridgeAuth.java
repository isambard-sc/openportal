// SPDX-FileCopyrightText: © 2026 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

package org.openportal;

import java.nio.charset.StandardCharsets;
import java.time.ZoneOffset;
import java.time.ZonedDateTime;
import java.time.format.DateTimeFormatter;
import java.util.Locale;
import java.util.UUID;
import org.bouncycastle.crypto.digests.Blake2bDigest;

/**
 * Signs a request to the bridge API.
 *
 * <p>This is the whole of the authentication, and the part with no room for
 * interpretation: get any byte of it wrong and the bridge answers {@code 401
 * Unauthorized} with nothing to say why. Every step below was checked against a
 * running bridge rather than read off a document - see the class's tests, which
 * carry the vectors.
 *
 * <p>Four things about it are easy to get wrong, and each has cost somebody an
 * afternoon:
 *
 * <ol>
 *   <li><b>The primitive is keyed BLAKE2b-256, not HMAC.</b> The bridge uses
 *       {@code orion::auth::authenticate}, which is BLAKE2b in its native keyed
 *       mode with a 32-byte digest. It is not HMAC-SHA512, and it is not BLAKE2b
 *       inside an HMAC construction, so {@link javax.crypto.Mac} cannot do it and
 *       neither can anything else in the JDK. Hence BouncyCastle.
 *   <li><b>The bytes signed are the canonical string's JSON encoding</b>, not the
 *       canonical string. The bridge's signing helper is generic over anything
 *       serialisable and serialises to JSON first, so what reaches BLAKE2b is the
 *       canonical string wrapped in double quotes with the newlines written as
 *       the two characters {@code \n}. See {@link #jsonEncode}.
 *   <li><b>Non-ASCII stays as UTF-8</b> in that JSON encoding - it is never
 *       {@code &#92;u}-escaped. A JSON library configured to escape non-ASCII (which
 *       several do by default) produces a signature the bridge rejects, and only
 *       for the requests that happen to carry an accented character.
 *   <li><b>The length prefixes are byte lengths.</b> {@code String.length()} in
 *       Java counts UTF-16 code units, so a body containing {@code café} would be
 *       prefixed one short. Encode, then measure - {@link #field}.
 * </ol>
 *
 * <p>None of this is secret-dependent, so it is all testable without a bridge:
 * given a key, a date and a nonce, the signature is a pure function of the
 * request.
 */
public final class BridgeAuth {

    /** The tag length orion's {@code auth} module produces, in bytes. */
    private static final int TAG_BYTES = 32;

    /**
     * The {@code Date} header format the bridge requires, and the same string
     * that goes into the signature. It must be GMT, and within five seconds of
     * the bridge's clock.
     */
    private static final DateTimeFormatter RFC_2822 =
            DateTimeFormatter.ofPattern("EEE, dd MMM yyyy HH:mm:ss 'GMT'", Locale.US);

    private BridgeAuth() {}

    /** The current time in the form the {@code Date} header and signature need. */
    public static String now() {
        return RFC_2822.format(ZonedDateTime.now(ZoneOffset.UTC));
    }

    /** A fresh nonce. Any unique string will do; the bridge remembers them for 30 seconds. */
    public static String nonce() {
        return UUID.randomUUID().toString();
    }

    /**
     * One field of the canonical string: its byte length, a colon, then the value.
     *
     * <p>The length prefix is what makes the canonical string unambiguous - no
     * field's content can be read as a field boundary - so it has to be the
     * length in bytes of what actually goes on the wire.
     */
    private static String field(String value) {
        return value.getBytes(StandardCharsets.UTF_8).length + ":" + value;
    }

    /**
     * The version 2 canonical string: seven length-prefixed fields, newline-joined.
     *
     * <p>Every field is present exactly once. An absent body or nonce is the
     * empty string, which is {@code 0:} rather than a missing line, so the shape
     * does not change with the request.
     */
    static String canonicalString(
            String protocol, String date, String function, String body, String nonce) {
        return String.join(
                "\n",
                field("openportal-sig-v2"),
                field(protocol),
                field("application/json"),
                field(date),
                field(function),
                field(body == null ? "" : body),
                field(nonce == null ? "" : nonce));
    }

    /**
     * JSON-encode a string exactly as {@code serde_json} does.
     *
     * <p>Written out rather than delegated to Jackson because the rule that
     * matters here is a negative one: non-ASCII characters are <b>not</b>
     * escaped. Quotes, backslashes and control characters are, and everything
     * else - including every accented letter and emoji - is emitted as itself
     * and encoded as UTF-8 by the caller.
     */
    static String jsonEncode(String value) {
        StringBuilder out = new StringBuilder(value.length() + 16).append('"');

        for (int i = 0; i < value.length(); i++) {
            char c = value.charAt(i);

            switch (c) {
                case '"' -> out.append("\\\"");
                case '\\' -> out.append("\\\\");
                case '\n' -> out.append("\\n");
                case '\r' -> out.append("\\r");
                case '\t' -> out.append("\\t");
                case '\b' -> out.append("\\b");
                case '\f' -> out.append("\\f");
                default -> {
                    if (c < 0x20) {
                        out.append(String.format("\\" + "u%04x", (int) c));
                    } else {
                        out.append(c);
                    }
                }
            }
        }

        return out.append('"').toString();
    }

    /**
     * The value for the {@code Authorization} header.
     *
     * @param key the 32 bytes from the bridge invite file
     * @param protocol {@code "get"} or {@code "post"}, lower case
     * @param date the value of the {@code Date} header, character for character
     * @param function the endpoint name without its slash, e.g. {@code "fetch_job"}
     * @param body the request body, or the empty string for a GET
     * @param nonce the value of the {@code X-Nonce} header, or the empty string
     */
    public static String authorization(
            byte[] key, String protocol, String date, String function, String body, String nonce) {
        String canonical = canonicalString(protocol, date, function, body, nonce);
        byte[] message = jsonEncode(canonical).getBytes(StandardCharsets.UTF_8);

        return "OpenPortal " + hex(blake2b256(key, message));
    }

    /** Keyed BLAKE2b-256, which is what the bridge means by "signed". */
    static byte[] blake2b256(byte[] key, byte[] message) {
        // The digest is stateful and not thread-safe, so it is built per call
        // rather than shared. Signing is nowhere near a hot path.
        Blake2bDigest digest = new Blake2bDigest(key, TAG_BYTES, null, null);
        digest.update(message, 0, message.length);

        byte[] tag = new byte[TAG_BYTES];
        digest.doFinal(tag, 0);

        return tag;
    }

    static String hex(byte[] bytes) {
        StringBuilder out = new StringBuilder(bytes.length * 2);

        for (byte b : bytes) {
            out.append(Character.forDigit((b >> 4) & 0xf, 16));
            out.append(Character.forDigit(b & 0xf, 16));
        }

        return out.toString();
    }

    /** Decode the hex key from a bridge invite file into its 32 bytes. */
    static byte[] unhex(String hex) {
        String trimmed = hex.trim();

        if (trimmed.length() % 2 != 0) {
            throw new IllegalArgumentException("a hex string has an even number of characters");
        }

        byte[] out = new byte[trimmed.length() / 2];

        for (int i = 0; i < out.length; i++) {
            int high = Character.digit(trimmed.charAt(2 * i), 16);
            int low = Character.digit(trimmed.charAt(2 * i + 1), 16);

            if (high < 0 || low < 0) {
                throw new IllegalArgumentException("not a hex string: " + trimmed);
            }

            out[i] = (byte) ((high << 4) | low);
        }

        return out;
    }
}
