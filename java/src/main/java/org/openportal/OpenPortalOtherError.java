// SPDX-FileCopyrightText: © 2026 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

package org.openportal;

/**
 * An error with no more specific class. What an unrecognised error message decodes to.
 */
public class OpenPortalOtherError extends OpenPortalError {

    private static final long serialVersionUID = 1L;

    public OpenPortalOtherError(String message) {
        super(message);
    }
}
