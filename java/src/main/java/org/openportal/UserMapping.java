// SPDX-FileCopyrightText: © 2026 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

package org.openportal;

/**
 * What a user is called <i>here</i>, against what the portal calls them:
 * {@code <username>.<project>.<portal>:<local_user>:<local_group>}.
 *
 * <p>{@code localUser} is two different things depending on who produced the
 * mapping. An account agent reports the <b>Unix account name</b> it created; a
 * portal reports the member's <b>email address</b>, because at portal level
 * there are no Unix accounts to name. Both travel in the same position, and the
 * string alone cannot say which it is - so an email address is accepted here
 * (an {@code @} would fail the mapping-target grammar outright), and a consumer
 * that is about to use the value as a Unix name, a path component or a command
 * operand must check for itself. See {@code project-portal-api.md} §4.2.
 */
public record UserMapping(UserIdentifier user, String localUser, String localGroup)
        implements OpenPortalType {

    public UserMapping {
        localUser = requireLocalUser(localUser);
        localGroup = Identifiers.mappingTarget(localGroup, "local_group");
    }

    /**
     * Whether {@link #localUser} is an email address rather than a Unix name.
     *
     * <p>Neither form can contain the other's disqualifying characters, so this
     * is decidable - but it has to be asked, not assumed.
     */
    public boolean localUserIsEmail() {
        return localUser.contains("@");
    }

    /**
     * {@link #localUser} as a Unix name, refusing an email address.
     *
     * <p>Call this - not the accessor - anywhere the value becomes a Unix
     * account name, a path component or a command operand.
     */
    public String unixLocalUser() {
        if (localUserIsEmail()) {
            throw new IllegalStateException("local_user '" + localUser
                    + "' is an email address, not a Unix account name");
        }

        return localUser;
    }

    private static String requireLocalUser(String value) {
        if (value != null && value.contains("@")) {
            // An email address, which the mapping-target grammar rejects. Held
            // to the email rules instead: a length cap and a valid domain.
            return Email.validate(value);
        }

        return Identifiers.mappingTarget(value, "local_user");
    }

    public static UserMapping parse(String value) {
        String[] parts = Identifiers.splitMapping(value, 3, "UserMapping");

        return new UserMapping(UserIdentifier.parse(parts[0]), parts[1], parts[2]);
    }

    @Override
    public String typeName() {
        return "UserMapping";
    }

    @Override
    public com.fasterxml.jackson.databind.JsonNode toJson() {
        return Json.text(toString());
    }

    @Override
    public String toString() {
        return user + ":" + localUser + ":" + localGroup;
    }
}
