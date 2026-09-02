// SPDX-FileCopyrightText: © 2026 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

package org.openportal;

/**
 * The email and domain grammars, kept apart from the identifier one.
 *
 * <p>An email address cannot meet the identifier rules - {@code @} alone is
 * disqualifying, and real addresses carry {@code +} - and widening those rules
 * to fit would have relaxed the charset for every consumer, including the ones
 * spawning processes. So the two grammars are separate, and the caller states
 * which it needs. Mirrors {@code templemeads/src/validate.rs} and
 * {@code DomainPattern::validate_domain_name}.
 */
final class Email {

    /** The longest address that fits RFC 5321 §4.5.3.1's 256-octet path. */
    static final int MAX_LENGTH = 254;

    /** RFC 5321 §4.5.3.1 caps the local part at 64 octets. */
    static final int MAX_LOCAL_PART_LENGTH = 64;

    /** RFC 1035 §2.3.4 caps a DNS label at 63. */
    static final int MAX_LABEL_LENGTH = 63;

    private Email() {}

    /** The address, unchanged, or an {@link IllegalArgumentException}. */
    static String validate(String value) {
        if (value == null || value.isEmpty()) {
            throw new IllegalArgumentException("Invalid email address - cannot be empty");
        }

        if (value.getBytes(java.nio.charset.StandardCharsets.UTF_8).length > MAX_LENGTH) {
            throw new IllegalArgumentException("Invalid email address - longer than "
                    + MAX_LENGTH + " characters '" + value + "'");
        }

        int at = value.indexOf('@');

        if (at < 0) {
            throw new IllegalArgumentException(
                    "Invalid email address - must contain an '@' '" + value + "'");
        }

        String localPart = value.substring(0, at);
        String domain = value.substring(at + 1);

        if (domain.indexOf('@') >= 0) {
            throw new IllegalArgumentException(
                    "Invalid email address - must contain exactly one '@' '" + value + "'");
        }

        validateLocalPart(localPart, value);
        validateEmailDomain(domain, value);

        return value;
    }

    private static void validateLocalPart(String localPart, String value) {
        if (localPart.isEmpty()) {
            throw new IllegalArgumentException(
                    "Invalid email address - empty local part '" + value + "'");
        }

        if (localPart.length() > MAX_LOCAL_PART_LENGTH) {
            throw new IllegalArgumentException("Invalid email address - local part longer than "
                    + MAX_LOCAL_PART_LENGTH + " characters '" + value + "'");
        }

        // A leading `-` for the same reason as in an identifier component: a
        // value that starts with a dash can be read as a flag by a spawned tool.
        if (localPart.startsWith("-")) {
            throw new IllegalArgumentException(
                    "Invalid email address - local part cannot start with '-' '" + value + "'");
        }

        for (int i = 0; i < localPart.length(); i++) {
            char c = localPart.charAt(i);
            boolean ok = (c >= 'A' && c <= 'Z')
                    || (c >= 'a' && c <= 'z')
                    || (c >= '0' && c <= '9')
                    || c == '.'
                    || c == '_'
                    || c == '-'
                    || c == '+';

            if (!ok) {
                throw new IllegalArgumentException("Invalid email address - local part contains an "
                        + "illegal character '" + c
                        + "' (allowed: A-Z, a-z, 0-9, '.', '_', '-', '+') '" + value + "'");
            }
        }

        if (localPart.startsWith(".") || localPart.endsWith(".")) {
            throw new IllegalArgumentException("Invalid email address - local part cannot start or "
                    + "end with '.' '" + value + "'");
        }

        if (localPart.contains("..")) {
            throw new IllegalArgumentException(
                    "Invalid email address - local part cannot contain '..' '" + value + "'");
        }
    }

    /**
     * The domain half of an address, which needs at least two labels.
     *
     * <p>A bare hostname is not a routable address, and accepting one would
     * accept {@code user@localhost}-style values that mean different things on
     * different hosts.
     */
    private static void validateEmailDomain(String domain, String value) {
        if (domain.isEmpty()) {
            throw new IllegalArgumentException(
                    "Invalid email address - empty domain '" + value + "'");
        }

        String[] labels = domain.split("\\.", -1);

        if (labels.length < 2) {
            throw new IllegalArgumentException("Invalid email address - domain must have at least "
                    + "two labels '" + value + "'");
        }

        for (String label : labels) {
            validateLabel(label, "email domain", value);
        }
    }

    /**
     * A domain name for a {@link DomainPattern} - one or more labels.
     *
     * <p>Deliberately <i>not</i> the two-label rule above: a pattern's domain
     * half comes from {@code *.example.com} as {@code example.com}, and the
     * Rust side applies only the per-label rules here.
     */
    static String validateDomainName(String domain) {
        if (domain == null || domain.isEmpty()) {
            throw new IllegalArgumentException("Domain name cannot be empty");
        }

        for (String label : domain.split("\\.", -1)) {
            if (label.isEmpty()) {
                throw new IllegalArgumentException(
                        "Domain name cannot have empty labels (e.g., '..', '.com')");
            }

            validateLabel(label, "domain", domain);
        }

        return domain;
    }

    private static void validateLabel(String label, String what, String value) {
        if (label.length() > MAX_LABEL_LENGTH) {
            throw new IllegalArgumentException("Invalid " + what + " - label longer than "
                    + MAX_LABEL_LENGTH + " characters '" + value + "'");
        }

        for (int i = 0; i < label.length(); i++) {
            char c = label.charAt(i);
            boolean ok = (c >= 'A' && c <= 'Z')
                    || (c >= 'a' && c <= 'z')
                    || (c >= '0' && c <= '9')
                    || c == '-';

            if (!ok) {
                throw new IllegalArgumentException("Invalid " + what
                        + " - label '" + label + "' contains an illegal character '" + c
                        + "' (only letters, digits, and hyphens allowed)");
            }
        }

        if (label.startsWith("-") || label.endsWith("-")) {
            throw new IllegalArgumentException("Invalid " + what + " - label '" + label
                    + "' cannot start or end with a hyphen");
        }
    }
}
