// SPDX-FileCopyrightText: © 2026 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

//! A job's failure, with its kind kept separate from its sentence.
//!
//! A failure used to be a `String`, which meant every agent that wanted to act
//! on one - rather than merely log it - had to parse prose. The clearest case
//! is a site portal answering `create_award`: "awaiting approval" means keep
//! asking and "rejected" means stop, and the difference between them travelled
//! as a class-name prefix that both ends agreed on by convention.
//!
//! [`JobError`] gives that difference somewhere real to live. The prose is
//! still carried, unchanged, in the job's `result` field - nothing that reads it
//! today has to change - and the kind rides alongside in a new field that older
//! peers simply ignore. See `docs/plans/structured-errors-design.md`.
//!
//! # Who owns which kinds
//!
//! templemeads is domain-agnostic, so it defines only the kinds that belong to
//! the transport itself (the [`kind`] module below). A `Domain` contributes its
//! own vocabulary through [`crate::domain::Domain::error_kind_for`] -
//! `greatwestern` supplies the award decisions. A router hop in between carries
//! a kind it has never heard of without needing to understand it.

use serde::{Deserialize, Serialize};

/// The failure kinds that belong to the transport rather than to any `Domain`.
///
/// These are stable strings: they go on the wire and a peer may branch on them,
/// so treat a change to one as a breaking change.
pub mod kind {
    /// The job passed its expiry while still unfinished.
    pub const EXPIRED: &str = "expired";

    /// No agent could be found to handle the job.
    pub const UNROUTABLE: &str = "unroutable";

    /// The instruction is not implemented by the agent that received it.
    pub const UNSUPPORTED: &str = "unsupported";

    /// The job was refused before it ran - a failed authorisation check, an
    /// instruction that did not parse.
    pub const INVALID: &str = "invalid";

    /// The agent handling the job failed while running it, for a reason with no
    /// more specific kind.
    pub const RUN: &str = "run";

    /// A failure with no information about it at all. The honest answer when
    /// an older peer sent prose that nothing recognises.
    pub const UNKNOWN: &str = "unknown";
}

/// The legacy sentinel the portal agent wraps a downstream failure in.
const RUNTIME_ERROR_PREFIX: &str = "RuntimeError{";

/// The legacy sentinel for an expired job.
pub const EXPIRATION_ERROR: &str = "ExpirationError{}";

/// The legacy sentinel for a failure carrying no message.
pub const UNKNOWN_ERROR: &str = "UnknownError{}";

///
/// Why a job failed: a machine-readable `kind`, the human-readable `message`
/// that has always been carried, and optionally where it came from.
///
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JobError {
    /// A stable discriminant - one of [`kind`], or one contributed by the
    /// `Domain`. This is the field to branch on.
    kind: String,

    /// The human-readable detail. Identical to what the job's `result` field
    /// holds, so a peer that reads only that loses nothing.
    message: String,

    /// The agent the failure originated at. Diagnostic only, and deliberately
    /// optional: it names internal topology, so it is stripped before a job
    /// leaves the agent network - see [`JobError::redact_origin`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    origin: Option<String>,
}

impl JobError {
    /// Build an error of the given kind.
    pub fn new(kind: &str, message: &str) -> Self {
        Self {
            kind: kind.to_owned(),
            message: message.to_owned(),
            origin: None,
        }
    }

    /// Record where this failure originated. See [`Self::redact_origin`] for
    /// why this does not leave the agent network.
    pub fn with_origin(mut self, origin: &str) -> Self {
        self.origin = Some(origin.to_owned());
        self
    }

    /// The machine-readable kind. Branch on this.
    pub fn kind(&self) -> &str {
        &self.kind
    }

    /// The human-readable detail.
    pub fn message(&self) -> &str {
        &self.message
    }

    /// Where the failure originated, if recorded.
    pub fn origin(&self) -> Option<&str> {
        self.origin.as_deref()
    }

    /// True if this error is of the given kind.
    pub fn is_kind(&self, kind: &str) -> bool {
        self.kind == kind
    }

    /// Drop the origin.
    ///
    /// `origin` names an agent inside the network, which is useful while the
    /// failure is still being routed and is an information leak once it is
    /// handed to software outside it. The bridge applies this to everything it
    /// serves, so a connected portal never learns the internal topology from a
    /// failure it was sent.
    pub fn redact_origin(&mut self) {
        self.origin = None;
    }

    /// Strip the `RuntimeError{...}` wrapper the portal agent adds, if present.
    ///
    /// A prefix/suffix match rather than a character-set trim: the inner
    /// message can begin with any character, and trimming a set of characters
    /// would eat the start of it.
    pub fn unwrap_message(message: &str) -> &str {
        let message = message.trim();

        match message.strip_prefix(RUNTIME_ERROR_PREFIX) {
            Some(inner) => inner.strip_suffix('}').unwrap_or(inner).trim(),
            None => message,
        }
    }

    /// Infer a kind from a legacy error string.
    ///
    /// This is the bridge from prose to kinds, and it is what lets every
    /// existing `Job::errored` call site acquire a kind without being rewritten.
    /// `domain_kind` is the `Domain`'s own classification of the *unwrapped*
    /// message, which wins when it has an opinion - only the domain knows what
    /// its own failures mean.
    pub fn infer(message: &str, domain_kind: Option<&str>) -> Self {
        let unwrapped = Self::unwrap_message(message);

        if let Some(kind) = domain_kind {
            return Self::new(kind, message);
        }

        let kind = if unwrapped == EXPIRATION_ERROR || unwrapped.is_empty() && message.is_empty() {
            kind::EXPIRED
        } else if unwrapped == UNKNOWN_ERROR {
            kind::UNKNOWN
        } else if message.trim().starts_with(RUNTIME_ERROR_PREFIX) {
            // It was wrapped, so an agent downstream did fail while running -
            // we just have nothing more specific than that.
            kind::RUN
        } else {
            kind::UNKNOWN
        };

        Self::new(kind, message)
    }
}

impl std::fmt::Display for JobError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_domain_kind_wins_over_any_transport_guess() {
        // Only the domain knows that this prose means "ask me again later",
        // so its classification is not second-guessed.
        let e = JobError::infer(
            "RuntimeError{ManagedProjectPendingError: awaiting approval}",
            Some("award_pending"),
        );

        assert_eq!(e.kind(), "award_pending");
    }

    #[test]
    fn the_message_is_preserved_exactly() {
        // The prose is what older peers read. Inferring a kind must not alter
        // a single byte of it, wrapper included.
        let raw = "RuntimeError{ManagedProjectRejectedError: template not offered}";
        let e = JobError::infer(raw, Some("award_rejected"));

        assert_eq!(e.message(), raw);
    }

    #[test]
    fn legacy_sentinels_are_recognised() {
        assert_eq!(
            JobError::infer(EXPIRATION_ERROR, None).kind(),
            kind::EXPIRED
        );
        assert_eq!(JobError::infer(UNKNOWN_ERROR, None).kind(), kind::UNKNOWN);
    }

    #[test]
    fn a_wrapped_message_the_domain_cannot_place_is_a_run_failure() {
        let e = JobError::infer("RuntimeError{no such project}", None);

        assert_eq!(e.kind(), kind::RUN);
        assert_eq!(e.message(), "RuntimeError{no such project}");
    }

    #[test]
    fn unrecognised_prose_is_unknown_rather_than_a_guess() {
        let e = JobError::infer("something went wrong", None);

        assert_eq!(e.kind(), kind::UNKNOWN);
        assert_eq!(e.message(), "something went wrong");
    }

    #[test]
    fn unwrap_message_does_not_eat_the_start_of_the_message() {
        // The bug a character-set trim introduces: every leading character
        // that happens to appear in "RuntimeError{" gets removed.
        assert_eq!(
            JobError::unwrap_message("Runtime trouble in the engine room"),
            "Runtime trouble in the engine room"
        );
        assert_eq!(
            JobError::unwrap_message("RuntimeError{no such project}"),
            "no such project"
        );
        assert_eq!(
            JobError::unwrap_message("RuntimeError{bad }{ input}"),
            "bad }{ input"
        );
    }

    #[test]
    fn origin_is_optional_and_removable() {
        let mut e = JobError::new(kind::RUN, "boom").with_origin("portal.clusters.shared");
        assert_eq!(e.origin(), Some("portal.clusters.shared"));

        e.redact_origin();
        assert_eq!(e.origin(), None);
    }

    #[test]
    fn origin_is_absent_from_the_wire_when_unset() {
        let e = JobError::new(kind::RUN, "boom");
        let json = serde_json::to_string(&e).unwrap_or_else(|e| unreachable!("{:?}", e));

        assert!(!json.contains("origin"), "unset origin must not serialise");
    }

    #[test]
    fn a_peer_that_predates_origin_still_deserialises() {
        let json = r#"{"kind":"run","message":"boom"}"#;
        let e: JobError = serde_json::from_str(json).unwrap_or_else(|e| unreachable!("{:?}", e));

        assert_eq!(e.kind(), kind::RUN);
        assert_eq!(e.origin(), None);
    }

    #[test]
    fn an_unknown_kind_round_trips_untouched() {
        // A router hop carries a domain kind it has never heard of.
        let e = JobError::new("some_future_domain_kind", "a thing happened");
        let json = serde_json::to_string(&e).unwrap_or_else(|e| unreachable!("{:?}", e));
        let back: JobError =
            serde_json::from_str(&json).unwrap_or_else(|e| unreachable!("{:?}", e));

        assert_eq!(back, e);
    }
}
