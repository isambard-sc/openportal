// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use crate::grammar::{ProjectIdentifier, UserIdentifier};
use crate::Hpc;

use serde::{Deserialize, Serialize};
use std::fmt;
use templemeads::notification::Notification;
use templemeads::Error;

/// A fire-and-forget event notification. Unlike a Job, a Notification is not stored
/// on any board, carries no state machine, and no result is returned to the sender.
/// Analogous to UDP vs TCP for Jobs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum NotificationEvent {
    /// A user was successfully added to a system
    UserAdded(UserIdentifier),
    /// A user was removed from a system
    UserRemoved(UserIdentifier),
    /// A user's details were changed (e.g. home directory updated)
    UserChanged(UserIdentifier),
    /// A user was blocked from logging in
    UserBlocked(UserIdentifier),
    /// A previously blocked user was unblocked
    UserUnblocked(UserIdentifier),
    /// A project was added to a system
    ProjectAdded(ProjectIdentifier),
    /// A project was removed from a system
    ProjectRemoved(ProjectIdentifier),
    /// A project's details were changed
    ProjectChanged(ProjectIdentifier),
    /// All users in a project were blocked
    ProjectBlocked(ProjectIdentifier),
    /// All users in a project were unblocked
    ProjectUnblocked(ProjectIdentifier),
    /// An award (project) was created or registered in the web portal
    AwardAdded(ProjectIdentifier),
    /// An award (project) was removed from the web portal
    AwardRemoved(ProjectIdentifier),
    /// An award (project) was updated in the web portal
    AwardChanged(ProjectIdentifier),
    /// An award was accepted by the receiving portal
    AwardAccepted(ProjectIdentifier),
    /// An award was rejected by the receiving portal
    AwardRejected(ProjectIdentifier),
    /// Infrastructure-only: used by the bridge agent to ask the portal to forward
    /// an inner notification southbound, stripping the bridge from the path.
    /// Analogous to `Instruction::Submit` for Jobs. Not accepted by `parse()`.
    Forward(Box<Notification<Hpc>>),
}

impl NotificationEvent {
    pub fn parse(s: &str) -> Result<Self, Error> {
        let (event_name, rest) = match s.split_once(' ') {
            Some((e, r)) => (e, r.trim()),
            None => (s.trim(), ""),
        };

        match event_name {
            "user_added" => Ok(Self::UserAdded(UserIdentifier::parse(rest)?)),
            "user_removed" => Ok(Self::UserRemoved(UserIdentifier::parse(rest)?)),
            "user_changed" => Ok(Self::UserChanged(UserIdentifier::parse(rest)?)),
            "user_blocked" => Ok(Self::UserBlocked(UserIdentifier::parse(rest)?)),
            "user_unblocked" => Ok(Self::UserUnblocked(UserIdentifier::parse(rest)?)),
            "project_added" => Ok(Self::ProjectAdded(ProjectIdentifier::parse(rest)?)),
            "project_removed" => Ok(Self::ProjectRemoved(ProjectIdentifier::parse(rest)?)),
            "project_changed" => Ok(Self::ProjectChanged(ProjectIdentifier::parse(rest)?)),
            "project_blocked" => Ok(Self::ProjectBlocked(ProjectIdentifier::parse(rest)?)),
            "project_unblocked" => Ok(Self::ProjectUnblocked(ProjectIdentifier::parse(rest)?)),
            "award_added" => Ok(Self::AwardAdded(ProjectIdentifier::parse(rest)?)),
            "award_removed" => Ok(Self::AwardRemoved(ProjectIdentifier::parse(rest)?)),
            "award_changed" => Ok(Self::AwardChanged(ProjectIdentifier::parse(rest)?)),
            "award_accepted" => Ok(Self::AwardAccepted(ProjectIdentifier::parse(rest)?)),
            "award_rejected" => Ok(Self::AwardRejected(ProjectIdentifier::parse(rest)?)),
            "forward" => Err(Error::Parse(
                "NotificationEvent::Forward is an infrastructure-only event and cannot be parsed from a string".to_owned(),
            )),
            unknown => Err(Error::Parse(format!(
                "Unknown notification event: '{}'",
                unknown
            ))),
        }
    }
}

impl fmt::Display for NotificationEvent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UserAdded(u) => write!(f, "user_added {}", u),
            Self::UserRemoved(u) => write!(f, "user_removed {}", u),
            Self::UserChanged(u) => write!(f, "user_changed {}", u),
            Self::UserBlocked(u) => write!(f, "user_blocked {}", u),
            Self::UserUnblocked(u) => write!(f, "user_unblocked {}", u),
            Self::ProjectAdded(p) => write!(f, "project_added {}", p),
            Self::ProjectRemoved(p) => write!(f, "project_removed {}", p),
            Self::ProjectChanged(p) => write!(f, "project_changed {}", p),
            Self::ProjectBlocked(p) => write!(f, "project_blocked {}", p),
            Self::ProjectUnblocked(p) => write!(f, "project_unblocked {}", p),
            Self::AwardAdded(p) => write!(f, "award_added {}", p),
            Self::AwardRemoved(p) => write!(f, "award_removed {}", p),
            Self::AwardChanged(p) => write!(f, "award_changed {}", p),
            Self::AwardAccepted(p) => write!(f, "award_accepted {}", p),
            Self::AwardRejected(p) => write!(f, "award_rejected {}", p),
            Self::Forward(n) => write!(f, "forward [{}]", n),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_notification_event_parse_display_roundtrip() {
        #[allow(clippy::unwrap_used)]
        let user = UserIdentifier::parse("chris.project.brics").unwrap();
        let event = NotificationEvent::UserAdded(user);
        let s = event.to_string();
        #[allow(clippy::unwrap_used)]
        let parsed = NotificationEvent::parse(&s).unwrap();
        assert_eq!(event, parsed);
    }

    #[test]
    fn test_all_notification_events() {
        #[allow(clippy::unwrap_used)]
        let cases = vec![
            "user_added chris.project.brics",
            "user_removed chris.project.brics",
            "user_changed chris.project.brics",
            "user_blocked chris.project.brics",
            "user_unblocked chris.project.brics",
        ];
        for case in cases {
            #[allow(clippy::unwrap_used)]
            let event = NotificationEvent::parse(case).unwrap();
            assert_eq!(event.to_string(), case);
        }
    }

    #[test]
    fn test_project_notification_events() {
        #[allow(clippy::unwrap_used)]
        let cases = vec![
            "project_added myproject.brics",
            "project_removed myproject.brics",
            "project_changed myproject.brics",
            "project_blocked myproject.brics",
            "project_unblocked myproject.brics",
        ];
        for case in cases {
            #[allow(clippy::unwrap_used)]
            let event = NotificationEvent::parse(case).unwrap();
            assert_eq!(event.to_string(), case);
        }
    }

    #[test]
    fn test_unknown_event_errors() {
        let result = NotificationEvent::parse("nonexistent_event foo.bar.brics");
        assert!(result.is_err());
    }
}
