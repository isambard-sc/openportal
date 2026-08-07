// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use crate::agent::Type as AgentType;
use crate::agent_core::Config;
use crate::domain::Domain;
use crate::error::Error;

use crate::handler::{run_with_relay, set_my_service_details, set_verify_portal_ownership};
use crate::job::{Envelope, Job};
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
    run_instance(config, runner, true, false).await
}

///
/// Run the agent service for an Instance whose Jobs are *delegated* by another
/// agent rather than routed down from the portal that owns the identifiers they
/// name.
///
/// `op-cloudaccount` is the case this exists for: it is driven directly by
/// `op-cloudportal`, so its Jobs arrive on a `cloudportal.cloudaccount`
/// destination while their instructions name the upstream portal that owns the
/// project (e.g. `myproject.waldur`). The portal-ownership re-check that
/// [`run`] applies would therefore reject every such Job, correctly - the
/// property simply does not hold for this topology.
///
/// Prefer [`run`] unless your Instance is in that position: this variant gives
/// up a real defence, and the Jobs it accepts are bounded only by the
/// sender-adjacency check and by whatever the runner itself validates.
///
/// Unlike [`run`], this also honours `run --one-shot`, the same local
/// execute-and-exit mode `account`/`filesystem`/`scheduler` agents have: it
/// synthesizes a Job, runs it directly through `runner`, prints the result,
/// and exits - no network listener, no live peer connections. That's only
/// safe here because `run_delegated`'s one current user, `op-cloudaccount`,
/// answers every instruction locally against its own state; [`run`] is
/// shared with Instances (e.g. `op-cluster`) whose runners forward Jobs to
/// other agents, so it does not get one-shot support.
///
pub async fn run_delegated<L: Domain>(
    config: Config,
    runner: AsyncRunnable<L>,
) -> Result<(), Error> {
    run_instance(config, runner, false, true).await
}

async fn run_instance<L: Domain>(
    config: Config,
    runner: AsyncRunnable<L>,
    verify_portal_ownership: bool,
    support_one_shot: bool,
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

    if support_one_shot {
        if let Some(one_shot_commands) = config.one_shot_commands() {
            for one_shot_command in one_shot_commands {
                tracing::info!("Executing one-shot command: {}", one_shot_command);

                let job = Job::parse(
                    format!(
                        "{}.{} {}",
                        config.one_shot_sender(),
                        config.service().name(),
                        one_shot_command
                    )
                    .as_str(),
                    false,
                )?
                .pending()?;

                let envelope = Envelope::new(
                    &config.service().name(),
                    &config.one_shot_sender(),
                    &config.one_shot_zone(),
                    &job,
                );

                let job = runner(envelope).await?;

                let result = serde_json::from_str::<serde_json::Value>(&job.result_json()?);

                // now write this out as pretty-printed JSON
                match result {
                    Ok(json) => {
                        println!(
                            "{}",
                            serde_json::to_string_pretty(&json).unwrap_or_else(|_| {
                                "Failed to serialize result as pretty-printed JSON".to_string()
                            })
                        );
                    }
                    Err(_) => {
                        println!("{}", job.result_json()?);
                    }
                }
            }

            return Ok(());
        }
    }

    // run the Provider OpenPortal agent
    run_with_relay::<L>(config.service()).await?;

    Ok(())
}
