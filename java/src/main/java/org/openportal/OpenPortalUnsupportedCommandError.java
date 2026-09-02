// SPDX-FileCopyrightText: © 2026 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

package org.openportal;

/**
 * The instruction is not implemented by the portal that received it.<p>A portal implements as much of the contract as it has answers for; this distinguishes "I do not do that" from "that went wrong".
 */
public class OpenPortalUnsupportedCommandError extends OpenPortalError {

    private static final long serialVersionUID = 1L;

    public OpenPortalUnsupportedCommandError(String message) {
        super(message);
    }
}
