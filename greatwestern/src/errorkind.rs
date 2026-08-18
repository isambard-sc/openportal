// SPDX-FileCopyrightText: © 2026 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

//! This domain's own failure kinds, and how to recognise them in prose.
//!
//! templemeads owns the kinds that belong to the transport
//! (`templemeads::joberror::kind`); everything here is `greatwestern`
//! vocabulary, contributed through `Domain::error_kind_for`.
//!
//! The distinction that matters is between an award that is *pending* and one
//! that is *rejected*. An awarding portal retries the first for as long as it
//! stays pending and treats the second as final, so getting them the wrong way
//! round either strands an award that only needed approving, or retries forever
//! against a decision that will never change. See
//! `docs/specifications/project-portal-api.md` §3.3.
//!
//! # Why prose is still parsed
//!
//! A project portal is not an OpenPortal agent - it is Waldur, or a script,
//! answering over the bridge's HTTP API, and it reports a failure as a string.
//! [`classify`] is the one place that string becomes a kind. New portals should
//! keep sending the same class names; they are the stable, specified form.

/// This domain's failure kinds. Stable strings - they go on the wire and peers
/// branch on them, so treat a change as breaking.
pub mod kind {
    /// The award was accepted but is not in place yet, typically awaiting
    /// human approval. **Not a fault** - the caller is expected to ask again.
    pub const AWARD_PENDING: &str = "award_pending";

    /// The award was refused. Re-sending it unchanged will be refused again.
    pub const AWARD_REJECTED: &str = "award_rejected";

    /// An award decision with no more specific kind.
    pub const AWARD_PERMISSION: &str = "award_permission";
}

/// The class-name prefixes a project portal reports failures with, and the
/// kind each maps to.
///
/// Ordered so that a subclass is tried before the base it shares a prefix with:
/// `ManagedProjectRejectedError` and `ManagedProjectPermissionError` both begin
/// `ManagedProject`, and only an exact match on the whole class name separates
/// them.
const CLASS_KINDS: [(&str, &str); 4] = [
    ("ManagedProjectPendingError", kind::AWARD_PENDING),
    ("ManagedProjectRejectedError", kind::AWARD_REJECTED),
    ("ManagedProjectPermissionError", kind::AWARD_PERMISSION),
    (
        "OpenPortalUnsupportedCommandError",
        templemeads::joberror::kind::UNSUPPORTED,
    ),
];

/// Classify a failure message into one of this domain's kinds.
///
/// `message` has already had any `RuntimeError{...}` wrapper removed by
/// templemeads. Returns `None` when nothing here recognises it, which leaves
/// the transport's own inference to decide.
pub fn classify(message: &str) -> Option<&'static str> {
    let message = message.trim();

    for (class, kind) in CLASS_KINDS {
        // A bare class name, or the specified "<class>: <message>" form.
        // Nothing else counts: a class name that merely opens some free text
        // is not a classification.
        if message == class {
            return Some(kind);
        }

        if let Some(rest) = message.strip_prefix(class) {
            if rest.starts_with(": ") {
                return Some(kind);
            }
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_two_award_decisions_are_distinguished() {
        // The whole reason this module exists: one means retry, the other
        // means stop.
        assert_eq!(
            classify("ManagedProjectPendingError: awaiting approval"),
            Some(kind::AWARD_PENDING)
        );
        assert_eq!(
            classify("ManagedProjectRejectedError: template not offered"),
            Some(kind::AWARD_REJECTED)
        );
    }

    #[test]
    fn a_subclass_is_not_swallowed_by_its_base() {
        // Both begin "ManagedProject"; only the exact class name separates them.
        assert_eq!(
            classify("ManagedProjectPermissionError: no"),
            Some(kind::AWARD_PERMISSION)
        );
        assert_eq!(
            classify("ManagedProjectRejectedError: no"),
            Some(kind::AWARD_REJECTED)
        );
    }

    #[test]
    fn an_unsupported_command_maps_to_the_transport_kind() {
        // A portal declining an instruction it does not implement is a
        // transport-level fact, not an award decision.
        assert_eq!(
            classify("OpenPortalUnsupportedCommandError: no get_users here"),
            Some(templemeads::joberror::kind::UNSUPPORTED)
        );
    }

    #[test]
    fn a_bare_class_name_is_enough() {
        assert_eq!(
            classify("ManagedProjectPendingError"),
            Some(kind::AWARD_PENDING)
        );
    }

    #[test]
    fn free_text_that_merely_starts_with_a_class_name_is_not_classified() {
        assert_eq!(classify("ManagedProjectPendingErrors are common"), None);
        assert_eq!(classify("no such project"), None);
        assert_eq!(classify(""), None);
    }
}
