// SPDX-FileCopyrightText: © 2026 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

package org.openportal;

import java.io.IOException;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.regex.Matcher;
import java.util.regex.Pattern;

/**
 * The bridge invite file: where the bridge is, and the key to sign with.
 *
 * <p>{@code op-bridge bridge --config <file>} writes it, and it is a credential -
 * anything holding it can drive the portal. Transfer it as one.
 *
 * <pre>
 * url = "http://localhost:3000"
 *
 * [key]
 * data = "&lt;64 hex characters&gt;"
 * </pre>
 *
 * <p>The parser here is two regular expressions rather than a TOML library,
 * because this is the only TOML this client reads and the file has exactly two
 * values in it. Use your own TOML parser if you would rather; the shape above is
 * what {@code op-bridge} writes, and {@code key} has been both a bare string and
 * a table in different versions, so both are accepted.
 */
public record BridgeConfig(String url, byte[] key) {

    private static final Pattern URL = Pattern.compile("(?m)^\\s*url\\s*=\\s*\"([^\"]+)\"");

    /**
     * The key, whether written as {@code key = "…"} or as a {@code [key]} table
     * with a {@code data} field. Both spellings have been produced by
     * {@code op-bridge bridge --config}, and the difference is not one a caller
     * should have to care about.
     */
    private static final Pattern KEY =
            Pattern.compile("(?m)^\\s*(?:key|data)\\s*=\\s*\"([0-9a-fA-F]+)\"");

    /** Read an invite file written by {@code op-bridge bridge --config}. */
    public static BridgeConfig load(Path file) throws IOException {
        String toml = Files.readString(file);

        Matcher url = URL.matcher(toml);
        Matcher key = KEY.matcher(toml);

        if (!url.find()) {
            throw new IOException("no url in the bridge invite file " + file);
        }

        if (!key.find()) {
            throw new IOException("no key in the bridge invite file " + file);
        }

        byte[] bytes = BridgeAuth.unhex(key.group(1));

        // orion refuses a key shorter than 32 bytes, so a truncated file would
        // otherwise fail later with something about digest parameters.
        if (bytes.length < 32) {
            throw new IOException(
                    "the key in " + file + " is " + bytes.length + " bytes; it must be at least 32");
        }

        return new BridgeConfig(url.group(1).replaceAll("/+$", ""), bytes);
    }
}
