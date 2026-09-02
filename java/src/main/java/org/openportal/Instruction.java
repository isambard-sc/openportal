// SPDX-FileCopyrightText: © 2026 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

package org.openportal;

import java.util.ArrayList;
import java.util.List;

/**
 * The verb and arguments of a job's {@code command}, after the destination.
 *
 * <p>A command is {@code "<destination> <verb> <arguments...>"}, so
 * {@code "site.bridge.cluster1 create_project myaward1.allocator {…}"} is the
 * verb {@code create_project} with two arguments.
 *
 * <p><b>This is not the full instruction grammar.</b> The Rust side parses each
 * instruction into a typed enum and validates every identifier as it goes; this
 * splits the string and hands you the pieces. That is enough to implement the
 * site portal contract - dispatch on {@link #command}, then parse the arguments
 * you expect - but it means a malformed identifier reaches your handler instead
 * of being refused before it. Validate what you use.
 *
 * <p>The one rule worth knowing: an argument that begins with <code>{</code> or
 * <code>[</code> takes the rest of the command, because an {@code AwardDetails}
 * blob contains spaces. Every instruction in the contract has at most one such
 * argument and it is always last.
 */
public record Instruction(String command, List<String> arguments) {

    public Instruction {
        arguments = List.copyOf(arguments);
    }

    /** Parse the part of a job's {@code command} that follows the destination. */
    public static Instruction parse(String rest) {
        String trimmed = rest == null ? "" : rest.trim();

        if (trimmed.isEmpty()) {
            return new Instruction("", List.of());
        }

        List<String> parts = new ArrayList<>();
        int start = 0;

        while (start < trimmed.length()) {
            // A JSON argument runs to the end of the command.
            if (!parts.isEmpty() && (trimmed.charAt(start) == '{' || trimmed.charAt(start) == '[')) {
                parts.add(trimmed.substring(start));
                break;
            }

            int space = trimmed.indexOf(' ', start);

            if (space < 0) {
                parts.add(trimmed.substring(start));
                break;
            }

            parts.add(trimmed.substring(start, space));

            // Skip any run of spaces, so a doubled space is not an empty argument.
            start = space;
            while (start < trimmed.length() && trimmed.charAt(start) == ' ') {
                start++;
            }
        }

        return new Instruction(parts.get(0), parts.subList(1, parts.size()));
    }

    /** The argument at {@code index}, or the empty string if there is none. */
    public String argument(int index) {
        return index < arguments.size() ? arguments.get(index) : "";
    }

    @Override
    public String toString() {
        return arguments.isEmpty() ? command : command + " " + String.join(" ", arguments);
    }
}
