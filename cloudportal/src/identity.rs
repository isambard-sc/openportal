// SPDX-FileCopyrightText: © 2026 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

//! Turns an `AwardDetails` member email into a `UserIdentifier`/`UserMapping`.
//!
//! Email addresses aren't valid dot-delimited `UserIdentifier` segments -
//! they usually contain dots themselves (e.g. in the domain) - so the
//! local "username" segment is a sanitised form of the email, while
//! `UserMapping`'s `local_user` stays the real email, matching `op-portal`'s
//! own convention that the email *is* the portal-level "local username".
//!
//! This is a kludge, not a bijection: two different emails that sanitise
//! to the same string (e.g. `a.b@x.com` and `a_b@x.com`) would collide.
//! Acceptable for a rough prototype; worth revisiting if this hardens.

use greatwestern::grammar::{ProjectIdentifier, UserIdentifier, UserMapping};
use templemeads::Error;

fn sanitise_email(email: &str) -> String {
    email
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect()
}

pub fn user_identifier_for_email(
    project: &ProjectIdentifier,
    email: &str,
) -> Result<UserIdentifier, Error> {
    UserIdentifier::parse(&format!(
        "{}.{}.{}",
        sanitise_email(email),
        project.project(),
        project.portal()
    ))
}

pub fn user_mapping_for_email(
    project: &ProjectIdentifier,
    email: &str,
    local_group: &str,
) -> Result<UserMapping, Error> {
    let user = user_identifier_for_email(project, email)?;
    UserMapping::new(&user, email, local_group)
}
