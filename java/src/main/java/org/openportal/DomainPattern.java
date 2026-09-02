// SPDX-FileCopyrightText: © 2026 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

package org.openportal;

import com.fasterxml.jackson.databind.JsonNode;

/**
 * One entry of an award's {@code allowed_domains} list: a domain, a wildcard
 * domain, or one exact email address.
 *
 * <p>Three forms, and which one a pattern is decides which of the two match
 * methods answers at all:
 *
 * <ul>
 *   <li>{@code "example.com"} - exact domain, case-insensitive.
 *   <li>{@code "*.example.com"} - any subdomain at any depth, so
 *       {@code a.b.example.com} matches too. Note it does <b>not</b> match the
 *       bare {@code example.com}.
 *   <li>{@code "chris@example.com"} - one exact address. {@link #matches}
 *       always returns {@code false} for this form, and
 *       {@link #matchesEmail} always returns {@code false} for the other two.
 * </ul>
 *
 * <p>A bare string on the wire.
 */
public record DomainPattern(String pattern) implements OpenPortalType {

    public DomainPattern {
        if (pattern == null || pattern.isEmpty()) {
            throw new IllegalArgumentException("Domain pattern cannot be empty");
        }

        if (pattern.contains("@")) {
            Email.validate(pattern);
        } else if (pattern.startsWith("*.")) {
            String domain = pattern.substring(2);

            if (domain.isEmpty()) {
                throw new IllegalArgumentException(
                        "Wildcard pattern must have a domain after '*.'");
            }

            if (domain.contains("*")) {
                throw new IllegalArgumentException(
                        "Wildcard '*' can only appear at the start of the pattern");
            }

            Email.validateDomainName(domain);
        } else {
            if (pattern.contains("*")) {
                throw new IllegalArgumentException(
                        "Wildcard '*' can only appear at the start as '*.'");
            }

            Email.validateDomainName(pattern);
        }
    }

    public static DomainPattern parse(String value) {
        return new DomainPattern(value);
    }

    /** Whether this names one address rather than a domain. */
    public boolean isEmailPattern() {
        return pattern.contains("@");
    }

    /** Whether a bare domain matches. Always {@code false} for an email pattern. */
    public boolean matches(String domain) {
        if (isEmailPattern() || domain == null) {
            return false;
        }

        String lower = domain.toLowerCase(java.util.Locale.ROOT);
        String self = pattern.toLowerCase(java.util.Locale.ROOT);

        if (self.startsWith("*.")) {
            // `*` spans dots, so this is "ends with .example.com" - which is
            // also why the bare domain does not match its own wildcard.
            return lower.endsWith(self.substring(1));
        }

        return lower.equals(self);
    }

    /** Whether a full address matches. Always {@code false} for a domain pattern. */
    public boolean matchesEmail(String email) {
        if (!isEmailPattern() || email == null) {
            return false;
        }

        return pattern.toLowerCase(java.util.Locale.ROOT)
                .equals(email.toLowerCase(java.util.Locale.ROOT));
    }

    @Override
    public String typeName() {
        return "DomainPattern";
    }

    @Override
    public JsonNode toJson() {
        return Json.text(pattern);
    }

    @Override
    public String toString() {
        return pattern;
    }
}
