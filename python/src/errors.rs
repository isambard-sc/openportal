// SPDX-FileCopyrightText: © 2026 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

//! The typed errors a project portal can return, and their wire encoding.
//!
//! A portal answers a bridge-board job either with a result or with an error,
//! and for several instructions the error *is* the answer - an award awaiting
//! human approval has no `ProjectMapping` to return, only a reason. The
//! awarding portal acts on which error it receives, so the class has to survive
//! the trip.
//!
//! Jobs carry a single error string, so the class travels inside it as a
//! `"<ClassName>: <message>"` prefix, and the portal agent wraps that once more
//! as `RuntimeError{...}` on the way back. [`encode`] and [`decode`] are the two
//! ends of that convention.
//!
//! This lived as hand-rolled classes and a string parser in
//! `waldur-mastermind`'s `src/waldur_openportal/op.py`; it belongs here, so both
//! sides of a portal-to-portal exchange agree by construction rather than by
//! two implementations happening to match. See
//! `docs/specifications/project-portal-api.md` §3.3.

use greatwestern::errorkind::kind as gw_kind;
use pyo3::create_exception;
use pyo3::exceptions::PyOSError;
use pyo3::prelude::*;
use templemeads::joberror::{kind, JobError};

create_exception!(
    openportal,
    OpenPortalError,
    PyOSError,
    "Base class for every OpenPortal error.\n\n\
     Derives from `OSError`, which is what this module raised for every failure \
     before the hierarchy existed, so `except OSError` still catches everything."
);

create_exception!(
    openportal,
    OpenPortalOtherError,
    OpenPortalError,
    "An error with no more specific class. What an unrecognised error message \
     decodes to."
);

create_exception!(
    openportal,
    OpenPortalUnsupportedCommandError,
    OpenPortalError,
    "The instruction is not implemented by the portal that received it.\n\n\
     A portal implements as much of the contract as it has answers for; this \
     distinguishes 'I do not do that' from 'that went wrong'."
);

create_exception!(
    openportal,
    ManagedProjectPermissionError,
    OpenPortalError,
    "Base class for the two award decisions - pending and rejected."
);

create_exception!(
    openportal,
    ManagedProjectRejectedError,
    ManagedProjectPermissionError,
    "The award was refused. Re-sending it unchanged will be refused again, so \
     the awarding portal records it as errored and stops retrying."
);

create_exception!(
    openportal,
    ManagedProjectPendingError,
    ManagedProjectPermissionError,
    "The award was accepted but is not in place yet - typically waiting on \
     human approval.\n\n\
     This is not a fault. The awarding portal is expected to ask again later, \
     and to keep asking for as long as the award stays pending."
);

/// The error classes that survive a round trip, in the order [`decode`] tries
/// them. Longest names first is not required - the match is exact on the part
/// before `": "` - but keeping subclasses next to their base makes the table
/// easier to read against the hierarchy above.
const CLASSES: [&str; 6] = [
    "ManagedProjectPendingError",
    "ManagedProjectRejectedError",
    "ManagedProjectPermissionError",
    "OpenPortalUnsupportedCommandError",
    "OpenPortalOtherError",
    "OpenPortalError",
];

/// The message used when an error is raised without one.
fn default_message(class: &str) -> &'static str {
    match class {
        "ManagedProjectPendingError" => "The project is pending.",
        "ManagedProjectRejectedError" => "The project is rejected.",
        "ManagedProjectPermissionError" => "The project is not permitted.",
        "OpenPortalUnsupportedCommandError" => "The command is not supported.",
        _ => "An unspecified error occurred.",
    }
}

/// Encode a class name and message into the wire form, `"<class>: <message>"`.
///
/// An empty message is replaced by the class's default, so the receiving side
/// always has something to show a human.
pub fn encode(class: &str, message: &str) -> String {
    let message = message.trim();

    if message.is_empty() {
        format!("{}: {}", class, default_message(class))
    } else {
        format!("{}: {}", class, message)
    }
}

/// Strip the `RuntimeError{...}` wrapper the portal agent adds, if present.
///
/// Deliberately a prefix/suffix match rather than a character-set trim: the
/// inner message can begin with any character, and trimming a character set
/// would eat the start of it.
fn unwrap_runtime_error(raw: &str) -> &str {
    let raw = raw.trim();

    match raw.strip_prefix("RuntimeError{") {
        Some(inner) => inner.strip_suffix('}').unwrap_or(inner).trim(),
        None => raw,
    }
}

/// Split a wire error message into its class name and message.
///
/// Returns the class name (one of [`CLASSES`]) and the message with the prefix
/// removed. A message carrying no recognised prefix is an
/// `OpenPortalOtherError` and is returned unchanged - nothing is discarded on
/// the guess that it might have been a prefix.
pub fn decode(raw: &str) -> (&'static str, String) {
    let inner = unwrap_runtime_error(raw);

    for class in CLASSES {
        if let Some(rest) = inner.strip_prefix(class) {
            // Only a genuine "<class>: <message>" separator counts, so a class
            // name that merely starts the free text is not mistaken for one.
            if let Some(message) = rest.strip_prefix(": ") {
                return (class, message.trim().to_string());
            }

            if rest.is_empty() {
                return (class, default_message(class).to_string());
            }
        }
    }

    ("OpenPortalOtherError", inner.to_string())
}

/// Build the Python exception for a structured [`JobError`].
///
/// The preferred path: the kind was decided by the agent that failed, so
/// nothing here has to read prose. [`to_pyerr`] is the fallback for a peer too
/// old to have sent one.
pub fn to_pyerr_from_kind(error: &JobError) -> PyErr {
    let message = error.message();

    // The prose still carries the class prefix for older readers; strip it so
    // an exception built from a kind below does not read
    // "ClassName: ClassName: ...".
    let (_, detail) = decode(message);

    match error.kind() {
        gw_kind::AWARD_PENDING => ManagedProjectPendingError::new_err(detail),
        gw_kind::AWARD_REJECTED => ManagedProjectRejectedError::new_err(detail),
        gw_kind::AWARD_PERMISSION => ManagedProjectPermissionError::new_err(detail),
        kind::UNSUPPORTED => OpenPortalUnsupportedCommandError::new_err(detail),
        _ => {
            // No class is tied to this kind - a transport kind such as
            // `expired`, or one a future domain added. The prose may still name
            // a class, though (`OpenPortalError: ...` does), so defer to it
            // rather than flattening everything to `OpenPortalOtherError`.
            // Anything it cannot place becomes `OpenPortalOtherError` with its
            // text intact, which is the same answer as before.
            to_pyerr(message)
        }
    }
}

/// Build the Python exception a wire error message describes.
///
/// The fallback path, for a failure carrying no structured kind. Prefer
/// [`to_pyerr_from_kind`].
pub fn to_pyerr(raw: &str) -> PyErr {
    let (class, message) = decode(raw);

    match class {
        "ManagedProjectPendingError" => ManagedProjectPendingError::new_err(message),
        "ManagedProjectRejectedError" => ManagedProjectRejectedError::new_err(message),
        "ManagedProjectPermissionError" => ManagedProjectPermissionError::new_err(message),
        "OpenPortalUnsupportedCommandError" => OpenPortalUnsupportedCommandError::new_err(message),
        "OpenPortalError" => OpenPortalError::new_err(message),
        _ => OpenPortalOtherError::new_err(message),
    }
}

/// Encode a Python exception instance into the wire form.
///
/// The exception's own class name is used, so a portal that subclasses one of
/// ours still reports something intelligible; only the six names in [`CLASSES`]
/// decode back to a specific type, and anything else arrives as
/// `OpenPortalOtherError` with the full text intact.
pub fn from_exception(exception: &Bound<'_, PyAny>) -> PyResult<String> {
    let class = exception
        .get_type()
        .name()
        .map(|name| name.to_string())
        .unwrap_or_else(|_| "OpenPortalOtherError".to_string());

    let message = exception.str().map(|s| s.to_string()).unwrap_or_default();

    Ok(encode(&class, &message))
}

/// Register the exception hierarchy on the module.
pub fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    let py = m.py();

    m.add("OpenPortalError", py.get_type::<OpenPortalError>())?;
    m.add(
        "OpenPortalOtherError",
        py.get_type::<OpenPortalOtherError>(),
    )?;
    m.add(
        "OpenPortalUnsupportedCommandError",
        py.get_type::<OpenPortalUnsupportedCommandError>(),
    )?;
    m.add(
        "ManagedProjectPermissionError",
        py.get_type::<ManagedProjectPermissionError>(),
    )?;
    m.add(
        "ManagedProjectRejectedError",
        py.get_type::<ManagedProjectRejectedError>(),
    )?;
    m.add(
        "ManagedProjectPendingError",
        py.get_type::<ManagedProjectPendingError>(),
    )?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_kind_with_no_class_defers_to_what_the_prose_names() {
        // `OpenPortalError` has no kind of its own, so the kind is `unknown`.
        // The prose still names the class, and flattening it to
        // `OpenPortalOtherError` would lose fidelity the fallback path keeps.
        let e = JobError::new(kind::UNKNOWN, "OpenPortalError: no such award");
        let (class, message) = decode(e.message());

        assert_eq!(class, "OpenPortalError");
        assert_eq!(message, "no such award");
    }

    #[test]
    fn encode_uses_the_class_default_when_no_message_is_given() {
        assert_eq!(
            encode("ManagedProjectPendingError", ""),
            "ManagedProjectPendingError: The project is pending."
        );
        assert_eq!(
            encode("ManagedProjectRejectedError", "   "),
            "ManagedProjectRejectedError: The project is rejected."
        );
    }

    #[test]
    fn encode_and_decode_round_trip() {
        for class in CLASSES {
            let encoded = encode(class, "something specific happened");
            let (decoded_class, message) = decode(&encoded);

            assert_eq!(decoded_class, class);
            assert_eq!(message, "something specific happened");
        }
    }

    #[test]
    fn decode_strips_the_portal_agents_runtime_error_wrapper() {
        let raw = "RuntimeError{ManagedProjectPendingError: awaiting approval}";
        let (class, message) = decode(raw);

        assert_eq!(class, "ManagedProjectPendingError");
        assert_eq!(message, "awaiting approval");
    }

    #[test]
    fn decode_keeps_the_whole_message_when_no_class_is_recognised() {
        // The message must not be trimmed on the guess that some leading text
        // was a prefix - this is the bug a character-set trim would introduce.
        let (class, message) = decode("RuntimeError{no such project}");

        assert_eq!(class, "OpenPortalOtherError");
        assert_eq!(message, "no such project");

        let (class, message) = decode("Runtime trouble in the engine room");
        assert_eq!(class, "OpenPortalOtherError");
        assert_eq!(message, "Runtime trouble in the engine room");
    }

    #[test]
    fn decode_does_not_mistake_a_class_name_inside_free_text_for_a_prefix() {
        let (class, message) = decode("OpenPortalErrors are not this shape");

        assert_eq!(class, "OpenPortalOtherError");
        assert_eq!(message, "OpenPortalErrors are not this shape");
    }

    #[test]
    fn decode_accepts_a_bare_class_name() {
        let (class, message) = decode("ManagedProjectRejectedError");

        assert_eq!(class, "ManagedProjectRejectedError");
        assert_eq!(message, "The project is rejected.");
    }

    #[test]
    fn decode_prefers_the_specific_subclass_over_its_base() {
        // Both names share a prefix; the exact match on "<class>: " is what
        // keeps them apart.
        let (class, _) = decode("ManagedProjectPermissionError: no");
        assert_eq!(class, "ManagedProjectPermissionError");

        let (class, _) = decode("ManagedProjectRejectedError: no");
        assert_eq!(class, "ManagedProjectRejectedError");
    }

    #[test]
    fn decode_survives_a_message_containing_the_wrapper_syntax() {
        let (class, message) = decode("RuntimeError{ManagedProjectRejectedError: bad }{ input}");

        assert_eq!(class, "ManagedProjectRejectedError");
        assert_eq!(message, "bad }{ input");
    }
}
