// SPDX-FileCopyrightText: © 2026 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

package org.openportal;

/**
 * The award was accepted but is not in place yet - typically waiting on human approval.<p>This is not a fault. The awarding portal is expected to ask again later, and to keep asking for as long as the award stays pending.
 */
public class ManagedProjectPendingError extends ManagedProjectPermissionError {

    private static final long serialVersionUID = 1L;

    public ManagedProjectPendingError(String message) {
        super(message);
    }
}
