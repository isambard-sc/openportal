// SPDX-FileCopyrightText: © 2026 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use anyhow::Result;
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

mod identity;
mod state;

use clap::{Parser, Subcommand};
use once_cell::sync::Lazy;
use tokio::sync::RwLock;

use greatwestern::grammar::Instruction::{
    CreateProject, GetAward, GetAwards, GetProject, GetProjectMapping, GetProjects,
    GetStorageReport, GetStorageReports, GetUsageReport, GetUsageReports, GetUsers, RemoveProject,
    UpdateProject,
};
use greatwestern::grammar::{DateRange, ProjectIdentifier};
use greatwestern::storagereport::{ProjectStorageReport, StorageReport};
use greatwestern::usagereport::{ProjectUsageReport, UsageReport};
use greatwestern::Hpc;
use templemeads::agent;
use templemeads::agent::portal::{process_args, run, Config, Defaults};
use templemeads::agent::Type as AgentType;
use templemeads::async_runnable;
use templemeads::notification::default_notify_runner;
use templemeads::portal_identifier::PortalIdentifier;
use templemeads::set_notify_runner;
use templemeads::Error;

type Envelope = templemeads::job::Envelope<Hpc>;
type Job = templemeads::job::Job<Hpc>;

const CLOUDACCOUNT_WAIT_TIME: u64 = 5;
const POLL_INTERVAL_SECS: u64 = 30;

/// `AwardDetails.template` value -> `op-cloudaccount` peer name.
static OFFERINGS: Lazy<RwLock<HashMap<String, String>>> = Lazy::new(|| RwLock::new(HashMap::new()));

/// Admin commands for the human-in-the-loop approval workflow (design doc
/// §7). These never touch the network themselves - see `run_cli_command`.
#[derive(Parser)]
#[command(name = "op-cloudportal", disable_help_subcommand = true)]
struct CliArgs {
    #[command(subcommand)]
    command: CliCommand,
}

#[derive(Subcommand)]
enum CliCommand {
    /// List Awards awaiting approval
    ListPending,
    /// Approve a pending Award - it will be provisioned on op-cloudaccount
    /// the next time the running `op-cloudportal run` process polls
    Approve {
        #[arg(long)]
        project: String,
    },
    /// Reject a pending Award
    Reject {
        #[arg(long)]
        project: String,
        #[arg(long)]
        reason: Option<String>,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    // start tracing
    templemeads::config::initialise_tracing();

    // our bespoke approve/reject/list-pending commands are handled
    // entirely separately from templemeads' own CLI (init/client/server/
    // run/...) - if the arguments don't match one of ours, fall through
    if let Ok(cli) = CliArgs::try_parse() {
        return run_cli_command(cli.command).await;
    }

    // start system monitoring
    templemeads::spawn_system_monitor::<Hpc>();

    // create the OpenPortal paddington defaults
    let defaults = Defaults::parse(
        Some("cloudportal".to_owned()),
        Some(config_dir().join("cloudportal-config.toml")),
        Some("ws://localhost:8050".to_owned()),
        Some("127.0.0.1".to_owned()),
        Some(8050),
        None,
        None,
        Some(AgentType::Portal),
    );

    // now parse the command line arguments to get the service configuration
    let config = match process_args(&defaults).await? {
        Some(config) => config,
        None => {
            // Not running the service, so can safely exit
            return Ok(());
        }
    };

    let state_dir: PathBuf = config
        .option(
            "state-dir",
            &config_dir().join("cloudportal-state").to_string_lossy(),
        )
        .into();

    state::initialise(&state_dir).await?;
    *OFFERINGS.write().await = parse_offerings(&config.option("offerings", ""));

    tracing::info!("Cloud portal state directory: {}", state_dir.display());
    tracing::info!(
        "Cloud portal offerings: {:?}",
        OFFERINGS.read().await.clone()
    );

    async_runnable! {
        ///
        /// Runnable function that will be called when a job is received
        /// by the agent
        ///
        pub async fn cloudportal_runner(envelope: Envelope) -> Result<Job, Error>
        {
            let job = envelope.job();

            match job.instruction() {
                CreateProject(project, details) => {
                    let offering = resolve_offering(&details).await?;
                    let mapping = state::create_award(&project, &details, &offering).await?;
                    job.completed(mapping)
                },
                UpdateProject(project, details) => {
                    let mapping = state::update_award(&project, &details).await?;
                    job.completed(mapping)
                },
                RemoveProject(project) => {
                    let mapping = state::remove_award(&project).await?;
                    job.completed(mapping)
                },
                GetProject(project) => {
                    let details = state::get_award(&project).await?;
                    job.completed(details)
                },
                GetAward(project) => {
                    let details = state::get_award(&project).await?;
                    job.completed(details)
                },
                GetAwards(portal) => {
                    let awards = state::get_awards(&portal).await?;
                    job.completed(awards)
                },
                GetProjects(portal) => {
                    let projects = state::get_projects(&portal).await?;
                    job.completed(projects)
                },
                GetProjectMapping(project) => {
                    let mapping = state::get_project_mapping(&project).await?;
                    job.completed(mapping)
                },
                GetUsers(project) => {
                    let users = state::get_users(&project).await?;
                    job.completed(users)
                },
                GetUsageReport(project, dates) => {
                    let report = forward_usage_report(&project, &dates).await?;
                    job.completed(report)
                },
                GetUsageReports(portal, dates) => {
                    let report = forward_usage_reports(&portal, &dates).await?;
                    job.completed(report)
                },
                GetStorageReport(project, _dates) => {
                    // cloud accounts don't have a POSIX-style filesystem/quota
                    // concept yet - an empty report is safer than erroring a
                    // caller that always asks for both usage and storage.
                    job.completed(ProjectStorageReport::new(&project))
                },
                GetStorageReports(portal, _dates) => {
                    job.completed(StorageReport::new(&portal))
                },
                _ => {
                    Err(Error::InvalidInstruction(
                        format!("Invalid instruction: {}. CloudPortal only supports Award/project instructions", job.instruction()),
                    ))
                }
            }
        }
    }

    set_notify_runner::<Hpc>(default_notify_runner).await?;

    // background provisioning poller - see design doc §7. Started before
    // `run()` so it runs alongside the normal job-handling event loop.
    tokio::spawn(provisioning_poll_loop());

    run(config, cloudportal_runner).await?;

    Ok(())
}

fn config_dir() -> PathBuf {
    dirs::config_local_dir()
        .unwrap_or(
            ".".parse()
                .expect("Could not parse fallback config directory."),
        )
        .join("openportal")
}

/// Parse `"aws:cloudaccount-aws,azure:cloudaccount-azure"` into a
/// template-name -> peer-name map.
fn parse_offerings(s: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();

    for entry in s.split(',') {
        let entry = entry.trim();

        if entry.is_empty() {
            continue;
        }

        if let Some(colon) = entry.find(':') {
            let name = entry[..colon].trim().to_string();
            let peer = entry[colon + 1..].trim().to_string();

            if !name.is_empty() && !peer.is_empty() {
                map.insert(name, peer);
            }
        }
    }

    map
}

/// Which cloud offering (`AwardDetails.template`) an Award targets - the
/// only thing that disambiguates which `op-cloudaccount` a create_project
/// should map to (design doc §4). Fails loudly if `template` is missing or
/// unrecognised - there's no sensible default cloud provider to fall back to.
async fn resolve_offering(details: &greatwestern::grammar::AwardDetails) -> Result<String, Error> {
    let template = details.template().ok_or_else(|| {
        Error::InvalidInstruction(
            "AwardDetails.template is required to select a cloud offering".to_string(),
        )
    })?;

    let offerings = OFFERINGS.read().await;

    if offerings.contains_key(template.name()) {
        Ok(template.name().to_string())
    } else {
        Err(Error::InvalidInstruction(format!(
            "Unknown cloud offering '{}' - configured offerings: {:?}",
            template.name(),
            offerings.keys().collect::<Vec<_>>()
        )))
    }
}

async fn resolve_cloudaccount_peer(project: &ProjectIdentifier) -> Result<agent::Peer, Error> {
    let offering = state::get_offering(project).await?;

    let peer_name = OFFERINGS
        .read()
        .await
        .get(&offering)
        .cloned()
        .ok_or_else(|| {
            Error::MissingAgent(format!(
                "No op-cloudaccount configured for offering '{}'",
                offering
            ))
        })?;

    agent::find(&peer_name, CLOUDACCOUNT_WAIT_TIME)
        .await
        .ok_or_else(|| {
            Error::MissingAgent(format!(
                "Could not find op-cloudaccount peer '{}'",
                peer_name
            ))
        })
}

async fn forward_usage_report(
    project: &ProjectIdentifier,
    dates: &DateRange,
) -> Result<ProjectUsageReport, Error> {
    let peer = match resolve_cloudaccount_peer(project).await {
        Ok(peer) => peer,
        Err(e) => {
            tracing::warn!(
                "Could not resolve op-cloudaccount peer for {}: {}. Returning an empty usage report.",
                project,
                e
            );
            return Ok(ProjectUsageReport::new(project));
        }
    };

    let me = agent::name().await;

    let job = Job::parse(
        &format!(
            "{}.{} get_usage_report {} {}",
            me,
            peer.name(),
            project,
            dates
        ),
        false,
    )?
    .put(&peer)
    .await?;

    match job.wait().await?.result::<ProjectUsageReport>() {
        Ok(Some(report)) => Ok(report),
        Ok(None) => Ok(ProjectUsageReport::new(project)),
        Err(e) => {
            tracing::warn!(
                "op-cloudaccount returned an error for {}'s usage report: {}. Returning an empty usage report.",
                project,
                e
            );
            Ok(ProjectUsageReport::new(project))
        }
    }
}

async fn forward_usage_reports(
    portal: &PortalIdentifier,
    dates: &DateRange,
) -> Result<UsageReport, Error> {
    let mut report = UsageReport::new(portal);

    for mapping in state::get_projects(portal).await? {
        let project_report = forward_usage_report(mapping.project(), dates).await?;
        report.set_report(project_report)?;
    }

    Ok(report)
}

async fn provisioning_poll_loop() {
    loop {
        tokio::time::sleep(Duration::from_secs(POLL_INTERVAL_SECS)).await;
        provision_approved_awards().await;
    }
}

async fn provision_approved_awards() {
    let records = match state::approved_unprovisioned().await {
        Ok(records) => records,
        Err(e) => {
            tracing::warn!("Could not list approved-but-unprovisioned Awards: {}", e);
            return;
        }
    };

    for record in records {
        if let Err(e) = provision_award(&record).await {
            tracing::warn!("Could not provision Award {}: {}", record.project(), e);
        }
    }
}

/// Actually provision an approved Award on `op-cloudaccount`: add_project
/// (idempotent, so safe to call every cycle) then add_user for each
/// not-yet-provisioned member. Safe to retry - a partial failure just
/// means the next poll picks up where this one left off.
async fn provision_award(record: &state::AwardRecord) -> Result<(), Error> {
    let peer = resolve_cloudaccount_peer(record.project()).await?;
    let me = agent::name().await;
    let project = record.project();

    let _: Option<greatwestern::grammar::ProjectMapping> = Job::parse(
        &format!("{}.{} add_project {}", me, peer.name(), project),
        false,
    )?
    .put(&peer)
    .await?
    .wait()
    .await?
    .result()?;

    for email in record.unprovisioned_members() {
        let user = identity::user_identifier_for_email(project, &email)?;

        let _: Option<greatwestern::grammar::UserMapping> =
            Job::parse(&format!("{}.{} add_user {}", me, peer.name(), user), false)?
                .put(&peer)
                .await?
                .wait()
                .await?
                .result()?;

        state::mark_provisioned(project, &email).await?;
    }

    Ok(())
}

async fn run_cli_command(command: CliCommand) -> Result<()> {
    let config_file = config_dir().join("cloudportal-config.toml");

    let config: Config = paddington::config::load(&config_file).map_err(|e| {
        anyhow::anyhow!(
            "Could not load cloudportal config from '{}': {}",
            config_file.display(),
            e
        )
    })?;

    let state_dir: PathBuf = config
        .option(
            "state-dir",
            &config_dir().join("cloudportal-state").to_string_lossy(),
        )
        .into();

    state::initialise(&state_dir).await?;

    match command {
        CliCommand::ListPending => {
            let pending = state::list_pending().await?;

            if pending.is_empty() {
                println!("No Awards awaiting approval.");
            } else {
                for record in pending {
                    let member_count = record.details().members().unwrap_or_default().len();
                    println!(
                        "{}  (offering: {}, name: {}, members: {})",
                        record.project(),
                        record.offering(),
                        record.details().name().unwrap_or_default(),
                        member_count
                    );
                }
            }
        }
        CliCommand::Approve { project } => {
            let project = ProjectIdentifier::parse(&project)?;
            state::approve(&project).await?;
            println!(
                "Approved {} - it will be provisioned next time the running \
                 `op-cloudportal run` process polls.",
                project
            );
        }
        CliCommand::Reject { project, reason } => {
            let project = ProjectIdentifier::parse(&project)?;
            state::reject(&project, reason.as_deref()).await?;
            println!("Rejected {}", project);
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;
    use greatwestern::grammar::ProjectTemplate;

    #[test]
    fn test_parse_offerings() {
        let offerings = parse_offerings("aws:cloudaccount-aws, azure:cloudaccount-azure");
        assert_eq!(offerings.get("aws"), Some(&"cloudaccount-aws".to_string()));
        assert_eq!(
            offerings.get("azure"),
            Some(&"cloudaccount-azure".to_string())
        );
        assert_eq!(offerings.len(), 2);
    }

    #[test]
    fn test_parse_offerings_ignores_malformed_entries() {
        let offerings = parse_offerings("aws:cloudaccount-aws,,malformed,:novalue,noname:");
        assert_eq!(offerings.len(), 1);
        assert!(offerings.contains_key("aws"));
    }

    #[tokio::test]
    async fn test_resolve_offering() {
        *OFFERINGS.write().await = parse_offerings("aws:cloudaccount-aws");

        let mut with_template = greatwestern::grammar::AwardDetails::new();
        with_template.set_template(ProjectTemplate::parse("aws").unwrap());
        assert_eq!(resolve_offering(&with_template).await.unwrap(), "aws");

        let mut unknown_template = greatwestern::grammar::AwardDetails::new();
        unknown_template.set_template(ProjectTemplate::parse("azure").unwrap());
        assert!(resolve_offering(&unknown_template).await.is_err());

        let no_template = greatwestern::grammar::AwardDetails::new();
        assert!(resolve_offering(&no_template).await.is_err());
    }
}
