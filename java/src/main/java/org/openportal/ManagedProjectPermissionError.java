// SPDX-FileCopyrightText: © 2026 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

package org.openportal;

/**
 * Base class for the two award decisions - pending and rejected.
 */
public class ManagedProjectPermissionError extends OpenPortalError {

    private static final long serialVersionUID = 1L;

    public ManagedProjectPermissionError(String message) {
        super(message);
    }
}
