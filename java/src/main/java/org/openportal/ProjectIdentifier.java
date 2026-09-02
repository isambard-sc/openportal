// SPDX-FileCopyrightText: © 2026 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

package org.openportal;

/**
 * A project, named as {@code <project>.<portal>}.
 *
 * <p>Two components, always. The awarding portal's own identifier for a project
 * arrives in this form and the site is expected to hand back its <i>own</i>
 * name for the same thing in a {@link ProjectMapping} - the two are not
 * interchangeable, and a report that mixes them is a report the allocator
 * cannot attribute.
 *
 * <p>On the wire this is a bare string, not an object, both as a value and as a
 * map key.
 */
public record ProjectIdentifier(String project, String portal) implements OpenPortalType {

    public ProjectIdentifier {
        project = Identifiers.component(project, "project");
        portal = Identifiers.component(portal, "portal");
    }

    public static ProjectIdentifier parse(String value) {
        String[] parts = Identifiers.split(value, 2, "ProjectIdentifier");

        return new ProjectIdentifier(parts[0], parts[1]);
    }

    public PortalIdentifier portalIdentifier() {
        return new PortalIdentifier(portal);
    }

    @Override
    public String typeName() {
        return "ProjectIdentifier";
    }

    @Override
    public com.fasterxml.jackson.databind.JsonNode toJson() {
        return Json.text(toString());
    }

    @Override
    public String toString() {
        return project + "." + portal;
    }
}
