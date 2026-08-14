// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use crate::agent::{Peer, Type as AgentType};
use crate::command::Command;
use crate::domain::Domain;
use crate::error::Error;
use crate::job;

use anyhow::Result;
use paddington::command::Command as ControlCommand;

pub async fn process_control_message<L: Domain>(
    agent_type: &AgentType,
    command: ControlCommand,
) -> Result<(), Error> {
    match command {
        ControlCommand::Connected {
            agent,
            zone,
            engine: _,
            version: _,
        } => {
            let peer = Peer::new(&agent, &zone);
            tracing::info!("Connected to agent: {}", peer);
            Command::<L>::register(
                agent_type,
                env!("CARGO_PKG_NAME"),
                env!("CARGO_PKG_VERSION"),
                L::name(),
                L::version(),
            )
            .send_to(&peer)
            .await?;

            // now send the current board to the peer, so that they
            // can restore their state
            job::sync_board::<L>(&peer).await?;

            // now they have their new state, we need to send all of the
            // queued jobs for this peer
            job::send_queued::<L>(&peer).await?;
        }
        ControlCommand::Disconnect { agent, zone } => {
            let peer = Peer::new(&agent, &zone);
            tracing::warn!("Force disconnect from agent: {}", peer);
            paddington::disconnect(&agent, &zone).await?;
        }
        ControlCommand::Disconnected { agent, zone } => {
            let peer = Peer::new(&agent, &zone);
            tracing::info!("Disconnected from agent: {}", peer);

            // Drop the portal routes this peer told us about, so that a later
            // topology change does not present as a collision against a stale
            // route. Routes we originated from our own config are unaffected.
            // See `crate::portalroutes` and
            // `docs/plans/portal-route-discovery-design.md` §4.5.
            if crate::portalroutes::withdraw_all_from(&peer).await {
                crate::handler::withdraw_routes_from::<L>(&peer).await;
            }

            crate::agent::set_route_capable(&peer, false).await;
        }
        ControlCommand::Error { error } => {
            tracing::error!("Received error: {}", error);
        }
        ControlCommand::Watchdog { agent, zone } => {
            let peer = Peer::new(&agent, &zone);
            tracing::debug!("Received watchdog from agent: {}", peer);
            paddington::watchdog(&agent, &zone).await?;
        }
    }

    Ok(())
}
