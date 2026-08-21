// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use crate::agent::Type as AgentType;
use crate::agent_core::Config;
use crate::domain::Domain;
use crate::error::Error;

use crate::handler::{run_with_relay, set_my_service_details, set_verify_portal_ownership};
use crate::runnable::AsyncRunnable;
use anyhow::Result;

///
/// Run the agent service.
///
/// Jobs reaching an Instance are expected to be *portal-rooted*: the first agent
/// in a Job's destination should be the portal that owns the identifiers its
/// instruction names, because the Job was routed down from that portal through
/// the provider/platform layers. That is re-checked on receipt - see
/// `docs/specifications/security-review-2.md` (finding R34).
///
/// Use [`run_delegated`] instead for an Instance whose Jobs are handed to it by
/// another agent rather than routed down from the owning portal.
///
pub async fn run<L: Domain>(config: Config, runner: AsyncRunnable<L>) -> Result<(), Error> {
    run_instance(config, runner, true).await
}

///
/// Run the agent service for an Instance whose Jobs are *delegated* by another
/// agent rather than routed down from the portal that owns the identifiers they
/// name.
///
/// The case this exists for is an Instance driven directly by a peer that is
/// not the portal owning the identifiers: its Jobs arrive on a
/// `delegator.instance` destination while their instructions name the upstream
/// portal that owns the project (e.g. `myproject.waldur`). The
/// portal-ownership re-check that [`run`] applies would therefore reject every
/// such Job, correctly - the property simply does not hold for this topology.
///
/// No agent in this workspace currently uses it; it is kept for Instances
/// outside the tree that sit in that position.
///
/// Prefer [`run`] unless your Instance is in that position: this variant gives
/// up a real defence, and the Jobs it accepts are bounded only by the
/// sender-adjacency check and by whatever the runner itself validates.
///
pub async fn run_delegated<L: Domain>(
    config: Config,
    runner: AsyncRunnable<L>,
) -> Result<(), Error> {
    run_instance(config, runner, false).await
}

async fn run_instance<L: Domain>(
    config: Config,
    runner: AsyncRunnable<L>,
    verify_portal_ownership: bool,
) -> Result<(), Error> {
    if config.service().name().is_empty() {
        return Err(Error::Misconfigured("Service name is empty".to_string()));
    }

    if config.agent() != AgentType::Instance {
        return Err(Error::Misconfigured(
            "Service agent is not an Instance".to_string(),
        ));
    }

    // pass the service details onto the handler
    set_my_service_details(
        &config.service().name(),
        &config.agent(),
        Some(runner),
        true,
    )
    .await?;

    set_verify_portal_ownership::<L>(verify_portal_ownership).await?;

    // run the Provider OpenPortal agent
    run_with_relay::<L>(config.service()).await?;

    Ok(())
}
