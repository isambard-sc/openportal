// SPDX-FileCopyrightText: © 2026 David John <David.John@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use anyhow::Result;

mod client;
mod resources;
mod usage;

use resources::TenancyMode;

use greatwestern::grammar::Instruction::{
    AddLocalProject, AddLocalTenancyAddon, AddLocalUser, GetLocalLimit, GetLocalUsageReport,
    IsLocalProjectAdded, IsLocalProjectRemoved, IsLocalUserAdded, IsLocalUserRemoved,
    RemoveLocalProject, RemoveLocalTenancyAddon, RemoveLocalUser, SetLocalLimit, SetLocalTenancy,
};
use greatwestern::grammar::UserMapping;
use greatwestern::localids::LocalUserIds;
use greatwestern::tenancy::Tenancy;
use greatwestern::usagereport::Usage;
use greatwestern::Hpc;
use templemeads::agent::scheduler::{process_args, run, Defaults};
use templemeads::agent::{Peer, Type as AgentType};
use templemeads::async_runnable;
use templemeads::job::assert_not_expired;
use templemeads::notification::default_notify_runner;
use templemeads::set_notify_runner;
use templemeads::Error;

type Envelope = templemeads::job::Envelope<Hpc>;
type Job = templemeads::job::Job<Hpc>;

/// How many times to ask for a new user's numeric ids before giving up, and
/// the base delay between attempts (multiplied by the attempt number). The
/// account is created just before `add_local_user` arrives, and the name
/// service on the filesystem host can lag behind the identity service, so
/// the first ask can legitimately fail.
const USER_IDS_ATTEMPTS: u32 = 4;
const USER_IDS_BACKOFF: std::time::Duration = std::time::Duration::from_secs(5);

/// Seconds in one hour - `Usage` is stored (and exchanged with peers) in
/// seconds, but `Project.spec.budget.nodeHours` is a whole number of hours.
const SECONDS_PER_HOUR: u64 = 3600;

///
/// Main function for the Kubernetes agent application.
///
/// This agent connects to a single Kubernetes cluster and manages the
/// `Project`/`ProjectUser` custom resources that a KRO `ResourceGraphDefinition`
/// running on that cluster composes into namespaces, RBAC and quotas. It does
/// no provisioning itself - it only CRUDs those two objects.
///
#[tokio::main]
async fn main() -> Result<()> {
    // start tracing
    templemeads::config::initialise_tracing();

    // start system monitoring
    templemeads::spawn_system_monitor::<Hpc>();

    // create the OpenPortal paddington defaults
    let defaults: Defaults = Defaults::parse(
        Some("k8s".to_owned()),
        Some(
            dirs::config_local_dir()
                .unwrap_or(
                    ".".parse()
                        .expect("Could not parse fallback config directory."),
                )
                .join("openportal")
                .join("k8s-config.toml"),
        ),
        Some("ws://localhost:8049".to_owned()),
        Some("127.0.0.1".to_owned()),
        Some(8049),
        None,
        None,
        Some(AgentType::Scheduler),
    );

    // now parse the command line arguments to get the service configuration
    let config = match process_args(&defaults).await? {
        Some(config) => config,
        None => {
            // Not running the service, so can safely exit
            return Ok(());
        }
    };

    // These are only needed when NOT running in-cluster - `client::connect`
    // tries the pod's own ServiceAccount first and only falls back to these
    // if that fails. See `k8s/src/client.rs`.
    let api_server = config.option("k8s-api-server", "");
    let api_server = if api_server.is_empty() {
        None
    } else {
        Some(api_server)
    };

    let ca_bundle = config.option("k8s-ca-bundle", "ENTER_CA_BUNDLE_HERE");
    let ca_bundle = if ca_bundle.is_empty() || ca_bundle == "ENTER_CA_BUNDLE_HERE" {
        None
    } else {
        Some(ca_bundle)
    };

    let token = config.secret("k8s-token");

    client::connect(api_server.as_deref(), ca_bundle.as_deref(), token).await?;

    tracing::info!("Connected to the Kubernetes API server");

    let db_url = match config.secret("k8s-db-url") {
        Some(db_url) => db_url,
        None => {
            return Err(anyhow::anyhow!(
                "No usage-tracking database URL provided. Set this in the k8s-db-url \
                 option, e.g. postgres://readonly_user:password@host:5432/dbname"
                    .to_owned(),
            ));
        }
    };

    usage::connect(&db_url).await?;

    tracing::info!("Connected to the usage-tracking database");

    set_notify_runner::<Hpc>(default_notify_runner).await?;

    async_runnable! {
        ///
        /// Runnable function that will be called when a job is received
        /// by the agent
        ///
        pub async fn k8s_runner(envelope: Envelope) -> Result<Job, templemeads::Error>
        {
            let job = envelope.job();
            let me = envelope.recipient();
            let sender = envelope.sender();

            match job.instruction() {
                AddLocalProject(project) => {
                    client::upsert_project(&project).await?;
                    job.completed_none()
                },
                RemoveLocalProject(project) => {
                    client::delete_project(&project).await?;
                    job.completed_none()
                },
                AddLocalUser(user) => {
                    let ids = get_user_ids(me.name(), &sender, &user, job.expires()).await?;
                    client::upsert_project_user(&user, &ids).await?;
                    job.completed_none()
                },
                RemoveLocalUser(user) => {
                    client::delete_project_user(&user).await?;
                    job.completed_none()
                },
                IsLocalProjectAdded(mapping) => {
                    job.completed(client::project_exists(&mapping).await?)
                },
                IsLocalProjectRemoved(mapping) => {
                    job.completed(!client::project_exists(&mapping).await?)
                },
                IsLocalUserAdded(mapping) => {
                    job.completed(client::project_user_exists(&mapping).await?)
                },
                IsLocalUserRemoved(mapping) => {
                    job.completed(!client::project_user_exists(&mapping).await?)
                },
                GetLocalLimit(mapping) => {
                    let node_hours = client::get_project_node_hours(&mapping).await?;
                    job.completed(Usage::new(node_hours.saturating_mul(SECONDS_PER_HOUR)))
                }
                SetLocalLimit(mapping, limit) => {
                    let node_hours = limit.hours().max(0.0) as u64;
                    client::set_project_node_hours(&mapping, node_hours).await?;
                    job.completed(Usage::new(node_hours.saturating_mul(SECONDS_PER_HOUR)))
                }
                SetLocalTenancy(mapping, tenancy) => {
                    let mode = match tenancy {
                        Tenancy::Shared => TenancyMode::Shared,
                        Tenancy::VCluster => TenancyMode::VCluster,
                    };
                    client::set_project_tenancy(&mapping, mode).await?;
                    job.completed_none()
                }
                AddLocalTenancyAddon(mapping, addon) => {
                    client::set_project_tenancy_addon(&mapping, addon, true).await?;
                    job.completed_none()
                }
                RemoveLocalTenancyAddon(mapping, addon) => {
                    client::set_project_tenancy_addon(&mapping, addon, false).await?;
                    job.completed_none()
                }
                GetLocalUsageReport(mapping, dates) => {
                    let report = usage::get_usage_report(&mapping, &dates).await?;
                    job.completed(report)
                }
                _ => {
                    Err(Error::InvalidInstruction(
                        format!("Invalid instruction: {}. K8s agents do not support this instruction", job.instruction()),
                    ))
                }
            }
        }
    }

    run(config, k8s_runner).await?;

    Ok(())
}

///
/// Ask the agent that sent us this job (the cluster instance agent) for the
/// numeric ids of a local user. The instance agent relays this to the
/// filesystem agent, whose host resolves names through the same identity
/// service as the storage - this agent runs in a pod and cannot.
///
/// The lookup is retried a few times, because the account has normally only
/// just been created. If it still fails the job fails, rather than creating
/// a `ProjectUser` with no ids - `add_user` is idempotent, so it is safe for
/// the portal to retry the whole thing.
///
async fn get_user_ids(
    me: &str,
    sender: &Peer,
    mapping: &UserMapping,
    expires: &chrono::DateTime<chrono::Utc>,
) -> Result<LocalUserIds, Error> {
    let mut last_error = None;

    for attempt in 1..=USER_IDS_ATTEMPTS {
        assert_not_expired(expires)?;

        let result = async {
            let job = Job::parse(
                &format!("{}.{} get_local_user_ids {}", me, sender.name(), mapping),
                false,
            )?
            .put(sender)
            .await?;

            match job.wait().await?.result::<LocalUserIds>()? {
                Some(ids) => Ok(ids),
                None => Err(Error::MissingUser(format!(
                    "No ids were returned for user {}",
                    mapping
                ))),
            }
        }
        .await;

        match result {
            Ok(ids) => {
                tracing::info!("Resolved ids for user {}: {}", mapping, ids);
                return Ok(ids);
            }
            Err(e) => {
                tracing::warn!(
                    "Could not get the ids for user {} (attempt {} of {}): {}",
                    mapping,
                    attempt,
                    USER_IDS_ATTEMPTS,
                    e
                );
                last_error = Some(e);

                if attempt < USER_IDS_ATTEMPTS {
                    tokio::time::sleep(USER_IDS_BACKOFF.saturating_mul(attempt)).await;
                }
            }
        }
    }

    Err(last_error.unwrap_or_else(|| {
        Error::MissingUser(format!("Could not get the ids for user {}", mapping))
    }))
}
