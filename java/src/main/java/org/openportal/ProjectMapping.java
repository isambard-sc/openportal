// SPDX-FileCopyrightText: © 2026 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

package org.openportal;

/**
 * What a project is called <i>here</i>, against what the portal calls it:
 * {@code <project>.<portal>:<local_group>}.
 *
 * <p>This is the answer to {@code create_award} - see
 * {@code site-portal-api.md} §4.1. The awarding portal keeps the mapping and
 * uses it to read every later report, so the local name has to be the one the
 * site will actually keep using; returning a different one later strands the
 * award.
 */
public record ProjectMapping(ProjectIdentifier project, String localGroup)
        implements OpenPortalType {

    public ProjectMapping {
        localGroup = Identifiers.mappingTarget(localGroup, "local_group");
    }

    public static ProjectMapping parse(String value) {
        String[] parts = Identifiers.splitMapping(value, 2, "ProjectMapping");

        return new ProjectMapping(ProjectIdentifier.parse(parts[0]), parts[1]);
    }

    @Override
    public String typeName() {
        return "ProjectMapping";
    }

    @Override
    public com.fasterxml.jackson.databind.JsonNode toJson() {
        return Json.text(toString());
    }

    @Override
    public String toString() {
        return project + ":" + localGroup;
    }
}
