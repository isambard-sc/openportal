// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use crate::agent;
use crate::agent::Peer;
use crate::command::Command;
use crate::destination::Destination;
use crate::diagnostics;
use crate::domain::Domain;
use crate::error::Error;
use crate::handler::invoke_notify_runner;

use serde::{Deserialize, Serialize};
use std::fmt;
use std::future::Future;
use std::pin::Pin;
use uuid::Uuid;

/// A fire-and-forget notification routed along a destination path.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(bound = "")]
pub struct Notification<L: Domain> {
    id: Uuid,
    destination: Destination,
    event: L::NotificationEvent,
}

impl<L: Domain> Notification<L> {
    pub fn new(destination: Destination, event: L::NotificationEvent) -> Self {
        Self {
            id: Uuid::new_v4(),
            destination,
            event,
        }
    }

    /// Parse a notification string of the form:
    ///   `<destination> <event> [<argument>]`
    /// e.g. `brics.aip1.clusters.shared user_added chris.project.brics`
    pub fn parse(s: &str) -> Result<Self, Error> {
        let (dest_str, event_str) = s
            .split_once(' ')
            .ok_or_else(|| Error::Parse(format!("Notification missing event: '{}'", s)))?;
        let destination = Destination::parse(dest_str.trim())?;
        let event = L::parse_notification_event(event_str.trim())?;
        Ok(Self {
            id: Uuid::new_v4(),
            destination,
            event,
        })
    }

    pub fn id(&self) -> Uuid {
        self.id
    }

    pub fn destination(&self) -> &Destination {
        &self.destination
    }

    pub fn event(&self) -> &L::NotificationEvent {
        &self.event
    }
}

impl<L: Domain> fmt::Display for Notification<L> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} {}", self.destination, self.event)
    }
}

/// Routing envelope passed to a notify runner when a notification reaches its destination.
#[derive(Debug, Clone, PartialEq)]
pub struct NotificationEnvelope<L: Domain> {
    recipient: String,
    sender: String,
    zone: String,
    notification: Notification<L>,
}

impl<L: Domain> NotificationEnvelope<L> {
    pub fn new(
        recipient: &str,
        sender: &str,
        zone: &str,
        notification: &Notification<L>,
    ) -> Self {
        Self {
            recipient: recipient.to_owned(),
            sender: sender.to_owned(),
            zone: zone.to_owned(),
            notification: notification.clone(),
        }
    }

    pub fn recipient(&self) -> Peer {
        Peer::new(&self.recipient, &self.zone)
    }

    pub fn sender(&self) -> Peer {
        Peer::new(&self.sender, &self.zone)
    }

    pub fn notification(&self) -> &Notification<L> {
        &self.notification
    }
}

/// Function pointer type for notification handlers registered by agents.
pub type AsyncNotifyRunnable<L> =
    fn(NotificationEnvelope<L>) -> Pin<Box<dyn Future<Output = Result<(), Error>> + Send>>;

/// Send a notification to the next hop in `destination`.
///
/// Finds the agent immediately after the current agent in `destination` and
/// sends a `Notification` carrying `event` to it. Fire-and-forget: failures
/// are logged and counted but not propagated to the caller.
pub async fn send<L: Domain>(destination: &Destination, event: L::NotificationEvent) {
    let my_name = agent::name().await;
    let notification = Notification::<L>::new(destination.clone(), event);

    if destination.last() == my_name {
        // We are the final destination — deliver to our own notify runner.
        let self_peer = agent::get_self(None).await;
        let envelope = NotificationEnvelope::new(
            self_peer.name(),
            self_peer.name(),
            self_peer.zone(),
            &notification,
        );
        if let Err(e) = invoke_notify_runner(envelope).await {
            tracing::warn!(
                "Local notify runner error for [{}]: {}",
                notification.id(),
                e
            );
            diagnostics::increment_notification_failed().await;
        } else {
            diagnostics::increment_notification_sent().await;
        }
        return;
    }

    match destination.next(&my_name) {
        Some(next_name) => match agent::find(&next_name, 0).await {
            Some(peer) => {
                if let Err(e) = Command::notify(&notification).send_to(&peer).await {
                    tracing::warn!(
                        "Could not send notification [{}] to {}: {}",
                        notification.id(),
                        peer,
                        e
                    );
                    diagnostics::increment_notification_failed().await;
                } else {
                    diagnostics::increment_notification_sent().await;
                }
            }
            None => {
                tracing::warn!(
                    "Cannot send notification: upstream agent '{}' not found",
                    next_name
                );
                diagnostics::increment_notification_failed().await;
            }
        },
        None => {
            tracing::warn!(
                "Cannot send notification: '{}' not found in destination '{}'",
                my_name,
                destination
            );
            diagnostics::increment_notification_failed().await;
        }
    }
}

/// Default notify runner — logs the notification and does nothing else.
pub fn default_notify_runner<L: Domain>(
    envelope: NotificationEnvelope<L>,
) -> Pin<Box<dyn Future<Output = Result<(), Error>> + Send>> {
    Box::pin(async move {
        tracing::info!(
            "Notification [{}] from {} : {}",
            envelope.notification().id(),
            envelope.notification().destination(),
            envelope.notification().event()
        );
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use crate::test_domain::TestDomain;

    type Notification = super::Notification<TestDomain>;

    // Tests exercising real notification event variants (user_added,
    // project_blocked, ...) live alongside the domain crate's own grammar
    // tests instead of here - templemeads has no concrete NotificationEvent
    // to parse.

    #[test]
    fn test_notification_parse() {
        #[allow(clippy::unwrap_used)]
        let n = Notification::parse("brics.aip1.clusters.shared user_added chris.project.brics")
            .unwrap();
        assert_eq!(n.event().to_string(), "user_added chris.project.brics");
        assert_eq!(n.destination().to_string(), "brics.aip1.clusters.shared");
    }
}
