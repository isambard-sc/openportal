// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use anyhow::Result;

use templemeads::agent;
use templemeads::agent::portal::{process_args, run, Defaults};
use templemeads::agent::Type as AgentType;
use templemeads::async_runnable;

use templemeads::agent::Type::{Bridge, Portal};
use templemeads::grammar::Instruction::{
    CreateProject, GetProject, GetProjectMapping, GetUsageReport, Submit, UpdateProject,
};
use templemeads::grammar::{DateRange, ProjectDetails, ProjectIdentifier, ProjectMapping};
use templemeads::job::{Envelope, Job};
use templemeads::usagereport::UsageReport;
use templemeads::Error;

///
/// Main function for the portal instance agent
///
/// This purpose of this agent is to manage an individual instance
/// of a user and project management portal. It receives commands
/// from that portal that it forwards to other agents, and it can
/// send and receive commands from other portals to which it is
/// connected
///
#[tokio::main]
async fn main() -> Result<()> {
    // start tracing
    templemeads::config::initialise_tracing();

    // create the OpenPortal paddington defaults
    let defaults = Defaults::parse(
        Some("portal".to_owned()),
        Some(
            dirs::config_local_dir()
                .unwrap_or(
                    ".".parse()
                        .expect("Could not parse fallback config directory."),
                )
                .join("openportal")
                .join("portal-config.toml"),
        ),
        Some("ws://localhost:8040".to_owned()),
        Some("127.0.0.1".to_owned()),
        Some(8040),
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

    async_runnable! {
        ///
        /// Runnable function that will be called when a job is received
        /// by the portal. This creates a firewall between the agents
        /// south of the portal (which e.g. actually create accounts etc)
        /// the agents north of the portal (which e.g. create or query
        /// allocations) and the bridge agent to the east/west of the portal,
        /// which connects to the graphical portal user interface.
        ///
        pub async fn portal_runner(envelope: Envelope) -> Result<Job, Error>
        {
            let me = envelope.recipient();
            let mut job = envelope.job();

            let mut agent_is_bridge = false;
            let mut agent_is_portal = false;

            // Get information about the agent that sent this job
            // The only agents that can send jobs to a portal are
            // bridge agents, and other portal agents that have
            // expressly be configured to be given permission.
            // This permission is based on the zone of the portal to portal
            // connection
            match agent::agent_type(&envelope.sender()).await {
                Some(Bridge) => {
                    agent_is_bridge = true;
                }
                Some(Portal) => {
                    if !portal_to_portal_allowed(&envelope.sender(), &envelope.recipient()) {
                        return Err(Error::InvalidInstruction(
                            format!("Invalid instruction: {}. Portal {} is not allowed to send jobs to portal {}", job.instruction(), envelope.sender(), envelope.recipient()),
                        ));
                    }
                    agent_is_portal = true;
                }
                _ => {
                    return Err(Error::InvalidInstruction(
                        format!("Invalid instruction: {}. Only bridge agents can submit instructions to the portal", job.instruction()),
                    ));
                }
            }

            let sender = envelope.sender();

            // match instructions that can only be sent by bridge agents
            match job.instruction() {
                Submit(destination, instruction) => {
                    if !agent_is_bridge {
                        return Err(Error::InvalidInstruction(
                            format!("Invalid instruction: {}. Only bridge agents can submit instructions to the portal", job.instruction()),
                        ));
                    }

                    // This is a job that should have been received from
                    // the bridge, and which is to be interpreted and passed
                    // south-bound to the agents for processing
                    tracing::debug!("{} : {}", destination, instruction);
                    tracing::debug!("This was from {:?}", envelope);

                    if destination.agents().len() < 2 {
                        tracing::error!("Invalid instruction: {}. Destination must have at least two agents", job.instruction());
                        return Err(Error::InvalidInstruction(
                            format!("Invalid instruction: {}. Destination must have at least two agents", job.instruction()),
                        ));
                    }

                    // the first agent in the destination is the agent should be this portal
                    let first_agent = destination.agents()[0].clone();

                    if first_agent != envelope.recipient().name() {
                        tracing::error!("Invalid instruction: {}. First agent in destination should be this portal ({})", job.instruction(), envelope.recipient().name());
                        return Err(Error::InvalidInstruction(
                            format!("Invalid instruction: {}. First agent in destination should be this portal ({})",
                                        job.instruction(),
                                        envelope.recipient().name())
                        ));
                    }

                    // who is next in line to receive this job? - find it, and its zone
                    let next_agent = agent::find(&destination.agents()[1], 5).await.ok_or_else(|| {
                        tracing::error!("Invalid instruction: {}. Cannot find next agent in destination {}", job.instruction(), destination);
                        Error::InvalidInstruction(
                            format!("Invalid instruction: {}. Cannot find next agent in destination {}",
                                    job.instruction(), destination),
                        )
                    })?;

                    // create the job and send it to the board for the next agent
                    let southbound_job = Job::parse(&format!("{} {}", destination, instruction), true)?.put(&next_agent).await?;

                    job = job.running(Some("Job registered - processing...".to_string()))?;
                    job = job.update(&sender).await?;

                    // Wait for the submitted job to complete
                    let southbound_job = southbound_job.wait().await?;

                    if southbound_job.is_expired() {
                        tracing::error!("{} : {} : Error - job expired!", destination, instruction);
                        job = job.errored("ExpirationError{{}}")?;
                    } else if (southbound_job.is_error()) {
                        if let Some(message) = southbound_job.error_message() {
                            tracing::error!("{} : {} : Error - {}", destination, instruction, message);
                            job = job.errored(&format!("RuntimeError{{{}}}", message))?;
                        }
                        else {
                            tracing::error!("{} : {} : Error - unknown error", destination, instruction);
                            job = job.errored("UnknownError{{}}")?;
                        }
                    }
                    else {
                        tracing::info!("{} : {} : Success", destination, instruction);
                        job = job.copy_result_from(&southbound_job)?;
                    }

                    return Ok(job);
                }
                _ => {
                    if !(agent_is_portal || agent_is_bridge) {
                        return Err(Error::InvalidInstruction(
                            format!("Invalid instruction: {}. Only portal or bridge agents can send instructions to the portal", job.instruction()),
                        ));
                    }
                }
            }

            // match instructions that can be sent by portal agents
            match job.instruction() {
                CreateProject(project, details) => {
                    tracing::debug!("Creating project {} with details {}", project, details);

                    job.completed(
                        create_project(me.name(), &project, &details).await?)
                }
                UpdateProject(project, details) => {
                    tracing::debug!("Updating project {} with details {}", project, details);

                    job.completed(
                        update_project(me.name(), &project, &details).await?)
                }
                GetProject(project) => {
                    tracing::debug!("Getting project {}", project);

                    job.completed(
                        get_project(me.name(), &project).await?)
                }
                GetProjectMapping(project) => {
                    tracing::debug!("Getting project mapping for {}", project);

                    job.completed(
                        get_project_mapping(me.name(), &project).await?)
                }
                GetUsageReport(project, dates) => {
                    tracing::debug!("Getting usage report for {} for dates {}", project, dates);

                    job.completed(
                        get_usage_report(me.name(), &project, &dates).await?)
                }
                _ => {
                    tracing::error!("Invalid instruction: {}. Portal agents do not accept this instruction", job.instruction());
                    return Err(Error::InvalidInstruction(
                        format!("Invalid instruction: {}. Portal agents do not accept this instruction", job.instruction()),
                    ));
                }
            }
        }
    };

    // run the portal agent
    run(config, portal_runner).await?;

    Ok(())
}

const BRIDGE_WAIT_TIME: u64 = 5;

///
/// Return the zone that should be used for a portal to portal
/// connection where the sender has the ability to send jobs
/// to the recipient (but the recipient cannot send jobs to the sender)
///
fn portal_to_portal_zone(sender: &agent::Peer, recipient: &agent::Peer) -> String {
    format!("{}>{}", sender.name(), recipient.name())
}

///
/// Return whether or not the sender has permission to send jobs
/// to the recipient, assuming they are both portals
///
fn portal_to_portal_allowed(sender: &agent::Peer, recipient: &agent::Peer) -> bool {
    (sender.zone() == recipient.zone())
        && (sender.zone() == portal_to_portal_zone(sender, recipient))
}

///
/// Create a new project
///
pub async fn create_project(
    me: &str,
    project: &ProjectIdentifier,
    details: &ProjectDetails,
) -> Result<ProjectMapping, Error> {
    // we need to connect to our bridge agent, so it can be used
    // to tell the connected portal software to create the project.
    // This will return the ProjectIdentifier of the project that was
    // created, which we can then return as a ProjectMapping

    match agent::bridge(BRIDGE_WAIT_TIME).await {
        Some(bridge) => {
            // send the create_project job to the bridge agent
            let job = Job::parse(
                &format!(
                    "{}.{} create_project {} {}",
                    me,
                    bridge.name(),
                    project,
                    details
                ),
                false,
            )?
            .put(&bridge)
            .await?;

            // Wait for the add_job to complete
            let result = job.wait().await?.result::<ProjectMapping>()?;

            match result {
                Some(project) => {
                    tracing::debug!("Project created by bridge agent: {:?}", project);
                    Ok(project)
                }
                None => {
                    tracing::warn!("No project created?");
                    Err(Error::MissingProject(
                        "No project created by bridge agent".to_string(),
                    ))
                }
            }
        }
        None => {
            tracing::error!("No bridge agent found");
            Err(Error::MissingAgent(
                "Cannot run the job because there is no bridge agent".to_string(),
            ))
        }
    }
}

///
/// Update an existing project
///
pub async fn update_project(
    me: &str,
    project: &ProjectIdentifier,
    details: &ProjectDetails,
) -> Result<ProjectMapping, Error> {
    // we need to connect to our bridge agent, so it can be used
    // to tell the connected portal software to create the project.
    // This will return the ProjectIdentifier of the project that was
    // created, which we can then return as a ProjectMapping

    match agent::bridge(BRIDGE_WAIT_TIME).await {
        Some(bridge) => {
            // send the update_project job to the bridge agent
            let job = Job::parse(
                &format!(
                    "{}.{} update_project {} {}",
                    me,
                    bridge.name(),
                    project,
                    details
                ),
                false,
            )?
            .put(&bridge)
            .await?;

            // Wait for the add_job to complete
            let result = job.wait().await?.result::<ProjectMapping>()?;

            match result {
                Some(project) => {
                    tracing::debug!("Project created by bridge agent: {:?}", project);
                    Ok(project)
                }
                None => {
                    tracing::warn!("No project created?");
                    Err(Error::MissingProject(
                        "No project created by bridge agent".to_string(),
                    ))
                }
            }
        }
        None => {
            tracing::error!("No bridge agent found");
            Err(Error::MissingAgent(
                "Cannot run the job because there is no bridge agent".to_string(),
            ))
        }
    }
}

///
/// Get an existing project
///
pub async fn get_project(me: &str, project: &ProjectIdentifier) -> Result<ProjectDetails, Error> {
    // we need to connect to our bridge agent, so it can be used
    // to tell the connected portal software to create the project.
    // This will return the ProjectIdentifier of the project that was
    // created, which we can then return as a ProjectMapping

    match agent::bridge(BRIDGE_WAIT_TIME).await {
        Some(bridge) => {
            // send the get_project job to the bridge agent
            let job = Job::parse(
                &format!("{}.{} get_project {}", me, bridge.name(), project),
                false,
            )?
            .put(&bridge)
            .await?;

            // Wait for the add_job to complete
            let result = job.wait().await?.result::<ProjectDetails>()?;

            match result {
                Some(project) => {
                    tracing::debug!("Project retrieved by bridge agent: {:?}", project);
                    Ok(project)
                }
                None => {
                    tracing::warn!("No project retrieved?");
                    Err(Error::MissingProject(
                        "No project retrieved by bridge agent".to_string(),
                    ))
                }
            }
        }

        None => {
            tracing::error!("No bridge agent found");
            Err(Error::MissingAgent(
                "Cannot run the job because there is no bridge agent".to_string(),
            ))
        }
    }
}

///
/// Get the project mapping for an existing project
///
pub async fn get_project_mapping(
    me: &str,
    project: &ProjectIdentifier,
) -> Result<ProjectMapping, Error> {
    // we need to connect to our bridge agent, so it can be used
    // to tell the connected portal software to create the project.
    // This will return the ProjectIdentifier of the project that was
    // created, which we can then return as a ProjectMapping

    match agent::bridge(BRIDGE_WAIT_TIME).await {
        Some(bridge) => {
            // send the get_project_mapping job to the bridge agent
            let job = Job::parse(
                &format!("{}.{} get_project_mapping {}", me, bridge.name(), project),
                false,
            )?
            .put(&bridge)
            .await?;

            // Wait for the add_job to complete
            let result = job.wait().await?.result::<ProjectMapping>()?;

            match result {
                Some(mapping) => {
                    tracing::debug!("Project mapping retrieved by bridge agent: {:?}", mapping);
                    Ok(mapping)
                }
                None => {
                    tracing::warn!("No project mapping retrieved?");
                    Err(Error::MissingProject(
                        "No project mapping retrieved by bridge agent".to_string(),
                    ))
                }
            }
        }

        None => {
            tracing::error!("No bridge agent found");
            Err(Error::MissingAgent(
                "Cannot run the job because there is no bridge agent".to_string(),
            ))
        }
    }
}

///
/// Get the usage report for an existing project
///
pub async fn get_usage_report(
    me: &str,
    project: &ProjectIdentifier,
    dates: &DateRange,
) -> Result<UsageReport, Error> {
    // we need to connect to our bridge agent, so it can be used
    // to tell the connected portal software to create the project.
    // This will return the ProjectIdentifier of the project that was
    // created, which we can then return as a ProjectMapping

    match agent::bridge(BRIDGE_WAIT_TIME).await {
        Some(bridge) => {
            // send the get_usage_report job to the bridge agent
            let job = Job::parse(
                &format!(
                    "{}.{} get_usage_report {} {}",
                    me,
                    bridge.name(),
                    project,
                    dates
                ),
                false,
            )?
            .put(&bridge)
            .await?;

            // Wait for the add_job to complete
            let result = job.wait().await?.result::<UsageReport>()?;

            match result {
                Some(report) => {
                    tracing::debug!("Usage report retrieved by bridge agent: {:?}", report);
                    Ok(report)
                }
                None => {
                    tracing::warn!("No usage report retrieved?");
                    Err(Error::MissingProject(
                        "No usage report retrieved by bridge agent".to_string(),
                    ))
                }
            }
        }

        None => {
            tracing::error!("No bridge agent found");
            Err(Error::MissingAgent(
                "Cannot run the job because there is no bridge agent".to_string(),
            ))
        }
    }
}
