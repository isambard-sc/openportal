// SPDX-FileCopyrightText: © 2026 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use anyhow::Result;
use std::path::PathBuf;

mod accounting;
mod state;

use templemeads::agent::instance::{process_args, run, Defaults};
use templemeads::agent::Type as AgentType;
use templemeads::async_runnable;
use templemeads::grammar::Instruction::{
    AddProject, AddUser, BlockProject, BlockUser, GetLimit, GetProjectMapping, GetProjects,
    GetUsageReport, GetUsageReports, GetUserMapping, GetUsers, IsBlockedProject, IsBlockedUser,
    IsProtectedUser, RemoveProject, RemoveUser, SetLimit, UnblockProject, UnblockUser,
};
use templemeads::job::{Envelope, Job};
use templemeads::notification::{self, default_notify_runner, NotificationEvent};
use templemeads::set_notify_runner;
use templemeads::usagereport::UsageReport;
use templemeads::Error;

///
/// Main function for the cloud account agent.
///
/// This agent represents a single cloud account (e.g. an AWS account)
/// assigned to a project. There is no cloud-side API yet to record project
/// or user assignment, so this agent is the source of truth for that
/// (persisted as one JSON file per project, see `state.rs`). Usage/cost
/// data is never held as state - it is reconstructed on demand by parsing
/// whatever cost-report JSON files the cloud operators have dropped into
/// the accounting directory (see `accounting.rs`).
///
/// See `docs/plans/op-cloudaccount-design.md` for the full design.
///
#[tokio::main]
async fn main() -> Result<()> {
    // start tracing
    templemeads::config::initialise_tracing();

    // start system monitoring
    templemeads::spawn_system_monitor();

    // create the OpenPortal paddington defaults
    let defaults = Defaults::parse(
        Some("cloudaccount".to_owned()),
        Some(
            dirs::config_local_dir()
                .unwrap_or(
                    ".".parse()
                        .expect("Could not parse fallback config directory."),
                )
                .join("openportal")
                .join("cloudaccount-config.toml"),
        ),
        Some("ws://localhost:8049".to_owned()),
        Some("127.0.0.1".to_owned()),
        Some(8049),
        None,
        None,
        Some(AgentType::Instance),
    );

    // now parse the command line arguments to get the service configuration
    let config = match process_args(&defaults).await? {
        Some(config) => config,
        None => {
            // Not running the service, so can safely exit
            return Ok(());
        }
    };

    let config_dir = dirs::config_local_dir()
        .unwrap_or(
            ".".parse()
                .expect("Could not parse fallback config directory."),
        )
        .join("openportal");

    let state_dir: PathBuf = config
        .option(
            "state-dir",
            &config_dir.join("cloudaccount-state").to_string_lossy(),
        )
        .into();

    let accounting_dir: PathBuf = config
        .option(
            "accounting-dir",
            &config_dir.join("cloudaccount-accounting").to_string_lossy(),
        )
        .into();

    let currency = config.option("currency", "USD");

    state::initialise(&state_dir).await?;
    accounting::initialise(&accounting_dir, &currency).await?;

    tracing::info!("Cloud account state directory: {}", state_dir.display());
    tracing::info!(
        "Cloud account accounting directory: {}",
        accounting_dir.display()
    );
    tracing::info!("Cloud account currency: {}", currency);

    async_runnable! {
        ///
        /// Runnable function that will be called when a job is received
        /// by the agent
        ///
        pub async fn cloudaccount_runner(envelope: Envelope) -> Result<Job, Error>
        {
            let job = envelope.job();

            match job.instruction() {
                GetProjects(portal) => {
                    let mappings = state::get_projects(&portal).await?;
                    job.completed(mappings)
                },
                GetUsers(project) => {
                    let mappings = state::get_users(&project).await?;
                    job.completed(mappings)
                },
                AddProject(project) => {
                    let mapping = state::add_project(&project).await?;
                    notification::send(&envelope.job().destination().reverse(), NotificationEvent::ProjectAdded(project.clone())).await;
                    job.completed(mapping)
                },
                RemoveProject(project) => {
                    let mapping = state::remove_project(&project).await?;
                    notification::send(&envelope.job().destination().reverse(), NotificationEvent::ProjectRemoved(project.clone())).await;
                    job.completed(mapping)
                },
                AddUser(user) => {
                    let mapping = state::add_user(&user).await?;
                    notification::send(&envelope.job().destination().reverse(), NotificationEvent::UserAdded(user.clone())).await;
                    job.completed(mapping)
                },
                RemoveUser(user) => {
                    let mapping = state::remove_user(&user).await?;
                    notification::send(&envelope.job().destination().reverse(), NotificationEvent::UserRemoved(user.clone())).await;
                    job.completed(mapping)
                },
                BlockUser(user) => {
                    let mapping = state::block_user(&user).await?;
                    notification::send(&envelope.job().destination().reverse(), NotificationEvent::UserBlocked(user.clone())).await;
                    job.completed(mapping)
                },
                UnblockUser(user) => {
                    let mapping = state::unblock_user(&user).await?;
                    notification::send(&envelope.job().destination().reverse(), NotificationEvent::UserUnblocked(user.clone())).await;
                    job.completed(mapping)
                },
                IsBlockedUser(user) => {
                    let is_blocked = state::is_blocked_user(&user).await?;
                    job.completed(is_blocked)
                },
                BlockProject(project) => {
                    let mapping = state::block_project(&project).await?;
                    notification::send(&envelope.job().destination().reverse(), NotificationEvent::ProjectBlocked(project.clone())).await;
                    job.completed(mapping)
                },
                UnblockProject(project) => {
                    let mapping = state::unblock_project(&project).await?;
                    notification::send(&envelope.job().destination().reverse(), NotificationEvent::ProjectUnblocked(project.clone())).await;
                    job.completed(mapping)
                },
                IsBlockedProject(project) => {
                    let is_blocked = state::is_blocked_project(&project).await?;
                    job.completed(is_blocked)
                },
                IsProtectedUser(_user) => {
                    // there is no concept of a protected/system user on a cloud account
                    job.completed(false)
                },
                GetProjectMapping(project) => {
                    let mapping = state::get_project_mapping(&project).await?;
                    job.completed(mapping)
                },
                GetUserMapping(user) => {
                    let mapping = state::get_user_mapping(&user).await?;
                    job.completed(mapping)
                },
                GetUsageReport(project, dates) => {
                    let mut report = accounting::get_usage_report(&project, &dates).await?;
                    report.add_mappings(&state::get_users(&project).await?)?;
                    job.completed(report)
                },
                GetUsageReports(portal, dates) => {
                    let mut report = UsageReport::new(&portal);

                    for mapping in state::get_projects(&portal).await? {
                        let mut project_report = accounting::get_usage_report(mapping.project(), &dates).await?;
                        project_report.add_mappings(&state::get_users(mapping.project()).await?)?;
                        report.set_report(project_report)?;
                    }

                    job.completed(report)
                },
                GetLimit(project) => {
                    let limit = accounting::get_limit(&project).await?;
                    job.completed(limit)
                },
                SetLimit(project, _limit) => {
                    Err(Error::InvalidInstruction(
                        format!("Cannot set the limit for project {} - the cloud platform does not yet support this agent pushing a budget change.", project),
                    ))
                },
                _ => {
                    Err(Error::InvalidInstruction(
                        format!("Invalid instruction: {}. CloudAccount only supports project/user assignment and usage-report instructions", job.instruction()),
                    ))
                }
            }
        }
    }

    set_notify_runner(default_notify_runner).await?;
    run(config, cloudaccount_runner).await?;

    Ok(())
}
