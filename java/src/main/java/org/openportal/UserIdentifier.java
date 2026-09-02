// SPDX-FileCopyrightText: © 2026 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

package org.openportal;

/**
 * A user, named as {@code <username>.<project>.<portal>}.
 *
 * <p>Three components: a user is always named <i>within</i> a project, so the
 * same person on two projects is two identifiers. That is deliberate - usage is
 * attributed per project, and a report keyed by bare username could not say
 * which project consumed it.
 */
public record UserIdentifier(String username, String project, String portal)
        implements OpenPortalType {

    public UserIdentifier {
        username = Identifiers.component(username, "username");
        project = Identifiers.component(project, "project");
        portal = Identifiers.component(portal, "portal");
    }

    public static UserIdentifier parse(String value) {
        String[] parts = Identifiers.split(value, 3, "UserIdentifier");

        return new UserIdentifier(parts[0], parts[1], parts[2]);
    }

    public ProjectIdentifier projectIdentifier() {
        return new ProjectIdentifier(project, portal);
    }

    public PortalIdentifier portalIdentifier() {
        return new PortalIdentifier(portal);
    }

    @Override
    public String typeName() {
        return "UserIdentifier";
    }

    @Override
    public com.fasterxml.jackson.databind.JsonNode toJson() {
        return Json.text(toString());
    }

    @Override
    public String toString() {
        return username + "." + project + "." + portal;
    }
}
