// SPDX-FileCopyrightText: © 2026 David John <David.John@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use anyhow::Result;

mod client;
mod resources;

use greatwestern::grammar::Instruction::{
    AddLocalProject, AddLocalUser, GetLocalLimit, IsLocalProjectAdded, IsLocalProjectRemoved,
    IsLocalUserAdded, IsLocalUserRemoved, RemoveLocalProject, RemoveLocalUser, SetLocalLimit,
};
use greatwestern::usagereport::Usage;
use greatwestern::Hpc;
use templemeads::agent::scheduler::{process_args, run, Defaults};
use templemeads::agent::Type as AgentType;
use templemeads::async_runnable;
use templemeads::notification::default_notify_runner;
use templemeads::set_notify_runner;
use templemeads::Error;

type Envelope = templemeads::job::Envelope<Hpc>;
type Job = templemeads::job::Job<Hpc>;

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

    // get the extra options needed to connect to this cluster's API server
    let api_server = config.option("k8s-api-server", "");

    if api_server.is_empty() {
        return Err(anyhow::anyhow!(
            "No Kubernetes API server provided. Set this in the k8s-api-server option, \
             e.g. https://10.129.136.24:6443"
                .to_owned(),
        ));
    }

    let ca_bundle = config.option("k8s-ca-bundle", "ENTER_CA_BUNDLE_HERE");

    if ca_bundle == "ENTER_CA_BUNDLE_HERE" {
        return Err(anyhow::anyhow!(
            "No Kubernetes CA bundle provided. Set this in the k8s-ca-bundle option \
             to the PEM-encoded CA certificate(s) for this cluster's API server."
                .to_owned(),
        ));
    }

    let token = match config.secret("k8s-token") {
        Some(token) => token,
        None => {
            return Err(anyhow::anyhow!(
                "No Kubernetes service account token provided. Set this in the k8s-token \
                 option."
                    .to_owned(),
            ));
        }
    };

    client::connect(&api_server, &ca_bundle, token).await?;

    tracing::info!("Connected to the Kubernetes API server at {}", api_server);

    set_notify_runner::<Hpc>(default_notify_runner).await?;

    async_runnable! {
        ///
        /// Runnable function that will be called when a job is received
        /// by the agent
        ///
        pub async fn k8s_runner(envelope: Envelope) -> Result<Job, templemeads::Error>
        {
            let job = envelope.job();

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
                    client::upsert_project_user(&user).await?;
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
