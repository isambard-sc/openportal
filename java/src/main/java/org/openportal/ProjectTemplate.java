// SPDX-FileCopyrightText: © 2026 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

package org.openportal;

import com.fasterxml.jackson.databind.JsonNode;

/**
 * The name of the template an award asks for - the site's own vocabulary for a
 * kind of project ({@code "standard"}, {@code "large"}, {@code "gpu-cluster"}).
 *
 * <p>The names mean whatever the site says they mean; OpenPortal does not
 * interpret them. What matters is that a site must <b>publish</b> the templates
 * a resource accepts and <b>refuse</b> an award asking for one it does not
 * offer, with a {@link ManagedProjectRejectedError} - see
 * {@code site-portal-api.md}. Guessing a default instead silently provisions
 * the wrong thing.
 *
 * <p>A bare string on the wire, and inside {@code AwardDetails} the field is
 * spelt {@code template}, not {@code project_template}.
 */
public record ProjectTemplate(String name) implements OpenPortalType {

    public ProjectTemplate {
        name = name == null ? "" : name.trim();

        if (name.isEmpty()) {
            throw new IllegalArgumentException("Invalid ProjectTemplate - cannot be empty");
        }

        for (int i = 0; i < name.length(); i++) {
            char c = name.charAt(i);

            // `isLetterOrDigit`, not an ASCII range: the Rust side uses
            // `char::is_alphanumeric`, which is Unicode-wide.
            if (!Character.isLetterOrDigit(c) && c != '_' && c != '-') {
                throw new IllegalArgumentException("Invalid ProjectTemplate - can only contain "
                        + "alphanumeric characters, underscores and dashes '" + name + "'");
            }
        }
    }

    public static ProjectTemplate parse(String value) {
        return new ProjectTemplate(value);
    }

    @Override
    public String typeName() {
        return "ProjectTemplate";
    }

    @Override
    public JsonNode toJson() {
        return Json.text(name);
    }

    @Override
    public String toString() {
        return name;
    }
}
