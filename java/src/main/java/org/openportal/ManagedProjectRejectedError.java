// SPDX-FileCopyrightText: © 2026 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

package org.openportal;

/**
 * The award was refused. Re-sending it unchanged will be refused again, so the awarding portal records it as errored and stops retrying.
 */
public class ManagedProjectRejectedError extends ManagedProjectPermissionError {

    private static final long serialVersionUID = 1L;

    public ManagedProjectRejectedError(String message) {
        super(message);
    }
}
