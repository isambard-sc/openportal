// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use anyhow::Result;

use templemeads::agent;
use templemeads::agent::portal::{process_args, run, Defaults};
use templemeads::agent::Type as AgentType;
use templemeads::async_runnable;

use templemeads::agent::Type::Bridge;
use templemeads::destination::{Destination, Destinations};
use templemeads::grammar::Instruction::{
    AddOfferings, CreateProject, GetOfferings, GetProject, GetProjectMapping, GetProjects,
    GetUsageReport, GetUsageReports, RemoveOfferings, RemoveProject, Submit, SyncOfferings,
    UpdateProject,
};
use templemeads::grammar::{
    DateRange, PortalIdentifier, ProjectDetails, ProjectIdentifier, ProjectMapping,
};
use templemeads::job::{Envelope, Job};
use templemeads::usagereport::{ProjectUsageReport, UsageReport};
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
        pub async fn virtual_resource_runner(envelope: Envelope) -> Result<Job, Error>
        {
            let me = agent::name().await;
            let job = envelope.job();

            // the target resource is the last element in the destination path
            // this is the virtual resource that this portal manages
            let resource = job.destination().last().clone();

            // match instructions that can be sent to virtual resources
            match job.instruction() {
                CreateProject(project, details) => {
                    tracing::debug!("Creating project {} with details {}", project, details);

                    job.completed(
                        create_project(&me, &resource, &project, &details).await?)
                }
                RemoveProject(project) => {
                    tracing::debug!("Removing project {}", project);

                    // This is a special instruction that removes a project
                    // from the portal, and also removes the project from the
                    // bridge agent
                    job.completed(
                        remove_project(&me, &resource, &project).await?)
                }
                UpdateProject(project, details) => {
                    tracing::debug!("Updating project {} with details {}", project, details);

                    job.completed(
                        update_project(&me, &resource, &project, &details).await?)
                }
                GetProject(project) => {
                    tracing::debug!("Getting project {}", project);

                    job.completed(
                        get_project(&me, &resource, &project).await?)
                }
                GetProjects(portal) => {
                    tracing::debug!("Getting all projects");

                    // This is a special instruction that returns all projects
                    // that this portal has access to
                    job.completed(
                        get_projects(&me, &resource, &portal).await?)
                }
                GetProjectMapping(project) => {
                    tracing::debug!("Getting project mapping for {}", project);

                    job.completed(
                        get_project_mapping(&me, &resource, &project).await?)
                }
                GetUsageReport(project, dates) => {
                    tracing::debug!("Getting usage report for {} for dates {}", project, dates);

                    job.completed(
                        get_usage_report(&me, &resource, &project, &dates).await?)
                }
                GetUsageReports(portal, dates) => {
                    tracing::debug!("Getting usage reports for portal {}", portal);

                    // This is a special instruction that returns all usage reports
                    // that this portal has access to
                    job.completed(
                        get_usage_reports(&me, &resource, &portal, &dates).await?)
                }
                _ => {
                    tracing::error!("Invalid instruction: {}. Portal agents do not accept this instruction", job.instruction());
                    return Err(Error::InvalidInstruction(
                        format!("Invalid instruction: {}. Portal agents do not accept this instruction", job.instruction()),
                    ));
                }
            }
        }
    }

    async_runnable! {
        ///
        /// Runnable function that will be called when a job is received
        /// by the portal. This creates a firewall between the agents
        /// south of the portal (which e.g. actually create accounts etc)
        /// the agents north of the portal (which e.g. create or query
        /// allocations) and the bridge agent to the east/west of the portal,
        /// which connects to the graphical portal user interface.
        ///
        pub async fn portal_runner(envelope: Envelope) -> Result<Job, Error> {
            let mut job = envelope.job();
            let sender = envelope.sender();

            // match instructions that can only be sent by bridge agents
            match agent::agent_type(&envelope.sender()).await {
                Some(Bridge) => {
                    tracing::debug!("Received job from bridge agent: {}", job.instruction());

                    match job.instruction() {
                        Submit(destination, instruction) => {
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
                            let now = chrono::Utc::now();

                            let southbound_job = loop {
                                match southbound_job.try_wait(500).await? {
                                    Some(job) => {
                                        if job.is_finished() || job.is_expired() {
                                            break job;
                                        }
                                    }
                                    None => {
                                        let elapsed_secs = (chrono::Utc::now() - now).num_seconds();
                                        tracing::debug!("{} : {} : still waiting... ({} seconds)", destination, instruction, elapsed_secs);
                                    }
                                }

                                if southbound_job.is_expired() {
                                    break southbound_job;
                                }
                            };

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

                            Ok(job)
                        }
                        GetOfferings() => {
                            // This is a special instruction that returns the
                            // set of offerings that this portal manages
                            tracing::info!("Getting offerings");

                            job.completed(Destinations::default())
                        }
                        SyncOfferings(offerings) => {
                            // This is a special instruction that updates the
                            // set of offerings that this portal manages
                            tracing::info!("Syncing offerings to: {:?}", offerings);

                            job.completed(sync_offerings(&offerings).await?)
                        }
                        AddOfferings(offerings) => {
                            // This is a special instruction that adds to the
                            // set of offerings that this portal manages
                            tracing::info!("Adding offerings: {:?}", offerings);
                            job.completed(offerings)
                        }
                        RemoveOfferings(offerings) => {
                            // This is a special instruction that removes from the
                            // set of offerings that this portal manages
                            tracing::info!("Removing offerings: {:?}", offerings);
                            job.completed(offerings)
                        }
                        _ => {
                            Err(Error::InvalidInstruction(
                                format!("Invalid instruction: {}. Only bridge agents can send instructions to the portal", job.instruction()),
                            ))
                        }
                    }
                }
                _ => {
                    if agent::is_virtual(&envelope.recipient()).await {
                        // this is a request to send commands to a virtual resource
                        // managed by this portal
                        return virtual_resource_runner(envelope).await;
                    }

                    Err(Error::MissingAgent(
                        "Cannot run the job because the sender is not a bridge agent".to_string(),
                    ))
                }
            }
        }
    }

    // run the portal agent
    run(config, portal_runner).await?;

    Ok(())
}

const BRIDGE_WAIT_TIME: u64 = 5;

///
/// Synchronise the offerings to the passed set
///
pub async fn sync_offerings(offerings: &Destinations) -> Result<Destinations, Error> {
    let me = agent::name().await;

    match agent::bridge(BRIDGE_WAIT_TIME).await {
        Some(bridge) => {
            let mut synched_offerings = Vec::<Destination>::new();

            // first, make sure that we have virtual agents for each
            // of the offerings
            for offering in offerings.iter() {
                if offering.agents().len() != 3 {
                    tracing::error!(
                        "Invalid offering: {}. Offerings must have exactly three agents",
                        offering
                    );
                    continue;
                }

                // The offering's destination should be of the form
                // offering-name.local-portal.remote-portal
                if offering.agents()[1] != me {
                    tracing::error!(
                        "Invalid offering: {}. The second agent in the offering must be this portal ({})",
                        offering,
                        me
                    );
                    continue;
                }

                // the offering-name should be the name of the virtual resource,
                // while the zone should be remote-portal>local-portal, to
                // indicate that the remote portal can send instructions to
                // the virtual resource via this portal
                let resource = offering.agents()[0].clone();
                let zone = format!("{}>{}", offering.agents()[2], me);

                let peer = agent::Peer::new(&resource, &zone);

                if !agent::has_virtual(&peer).await {
                    tracing::info!(
                        "Registering virtual agent for offering {}. Agent {} in zone {}",
                        offering,
                        resource,
                        zone
                    );

                    agent::register_peer(
                        &agent::Peer::new(&resource, &zone),
                        &agent::Type::Virtual,
                        "virtual",
                        "virtual",
                    )
                    .await;
                }

                synched_offerings.push(offering.clone());
            }

            // now go through the virtual agents and remove any that
            // are not in the synched offerings
            let virtual_agents = agent::get_all(&agent::Type::Virtual).await;

            for virtual_agent in virtual_agents {
                // convert this back to a destination
                let zone = virtual_agent.zone().split('>').collect::<Vec<&str>>();

                let mut found = false;

                if zone.len() == 2 {
                    let destination = Destination::parse(&format!(
                        "{}.{}.{}",
                        virtual_agent.name(),
                        zone[1],
                        zone[0]
                    ))?;

                    found = synched_offerings.contains(&destination);
                }

                if !found {
                    tracing::info!(
                        "Removing virtual agent {} in zone {}",
                        virtual_agent.name(),
                        virtual_agent.zone()
                    );
                    agent::remove(&virtual_agent).await;
                }
            }

            // now sync the offerings with the bridge agent, so it
            // creates virtual agents to return results
            let sync_job = Job::parse(
                &format!(
                    "{}.{} sync_offerings {}",
                    me,
                    bridge.name(),
                    Destinations::new(&synched_offerings)
                ),
                false,
            )?;

            let sync_job = sync_job.put(&bridge).await?;

            // Wait for the sync_offerings to complete
            let result = sync_job.wait().await?.result::<Destinations>()?;

            if let Some(offerings) = result {
                tracing::info!("Offerings synched by bridge agent: {:?}", offerings);
            } else {
                tracing::warn!("No offerings synched?");
            }

            Ok(Destinations::new(&synched_offerings))
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
/// Create a new project
///
pub async fn create_project(
    me: &str,
    resource: &str,
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
                    "{}.{}.{} create_project {} {}",
                    me,
                    bridge.name(),
                    resource,
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
    resource: &str,
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
                    "{}.{}.{} update_project {} {}",
                    me,
                    bridge.name(),
                    resource,
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
/// Remove a project
///
pub async fn remove_project(
    me: &str,
    resource: &str,
    project: &ProjectIdentifier,
) -> Result<ProjectMapping, Error> {
    // we need to connect to our bridge agent, so it can be used
    // to tell the connected portal software to remove the project.
    // This will return the ProjectIdentifier of the project that was
    // removed, which we can then return as a ProjectMapping

    match agent::bridge(BRIDGE_WAIT_TIME).await {
        Some(bridge) => {
            // send the remove_project job to the bridge agent
            let job = Job::parse(
                &format!(
                    "{}.{}.{} remove_project {}",
                    me,
                    bridge.name(),
                    resource,
                    project
                ),
                false,
            )?
            .put(&bridge)
            .await?;

            // Wait for the remove_job to complete
            let result = job.wait().await?.result::<ProjectMapping>()?;

            match result {
                Some(project) => {
                    tracing::debug!("Project removed by bridge agent: {:?}", project);
                    Ok(project)
                }
                None => {
                    tracing::warn!("No project removed?");
                    Err(Error::MissingProject(
                        "No project removed by bridge agent".to_string(),
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
pub async fn get_project(
    me: &str,
    resource: &str,
    project: &ProjectIdentifier,
) -> Result<ProjectDetails, Error> {
    // we need to connect to our bridge agent, so it can be used
    // to tell the connected portal software to create the project.
    // This will return the ProjectIdentifier of the project that was
    // created, which we can then return as a ProjectMapping

    match agent::bridge(BRIDGE_WAIT_TIME).await {
        Some(bridge) => {
            // send the get_project job to the bridge agent
            let job = Job::parse(
                &format!(
                    "{}.{}.{} get_project {}",
                    me,
                    bridge.name(),
                    resource,
                    project
                ),
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
/// Get the full set of project mappings for all projects managed
/// for the remove portal
///
pub async fn get_projects(
    me: &str,
    resource: &str,
    portal: &PortalIdentifier,
) -> Result<Vec<ProjectMapping>, Error> {
    // we need to connect to our bridge agent, so it can be used
    // to tell the connected portal software to get the projects.

    match agent::bridge(BRIDGE_WAIT_TIME).await {
        Some(bridge) => {
            // send the get_projects job to the bridge agent
            let job = Job::parse(
                &format!(
                    "{}.{}.{} get_projects {}",
                    me,
                    bridge.name(),
                    resource,
                    portal
                ),
                false,
            )?
            .put(&bridge)
            .await?;

            // Wait for the get_projects to complete
            let result = job.wait().await?.result::<Vec<ProjectMapping>>()?;

            match result {
                Some(projects) => {
                    tracing::debug!("Projects retrieved by bridge agent: {:?}", projects);
                    Ok(projects)
                }
                None => {
                    tracing::warn!("No projects retrieved?");
                    Err(Error::MissingProject(
                        "No projects retrieved by bridge agent".to_string(),
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
    resource: &str,
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
                &format!(
                    "{}.{}.{} get_project_mapping {}",
                    me,
                    bridge.name(),
                    resource,
                    project
                ),
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
    resource: &str,
    project: &ProjectIdentifier,
    dates: &DateRange,
) -> Result<ProjectUsageReport, Error> {
    // we need to connect to our bridge agent, so it can be used
    // to tell the connected portal software to create the project.
    // This will return the ProjectIdentifier of the project that was
    // created, which we can then return as a ProjectMapping

    match agent::bridge(BRIDGE_WAIT_TIME).await {
        Some(bridge) => {
            // send the get_usage_report job to the bridge agent
            let job = Job::parse(
                &format!(
                    "{}.{}.{} get_usage_report {} {}",
                    me,
                    bridge.name(),
                    resource,
                    project,
                    dates
                ),
                false,
            )?
            .put(&bridge)
            .await?;

            // Wait for the add_job to complete
            let result = job.wait().await?.result::<ProjectUsageReport>()?;

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

///
/// Get the usage reports for all projects managed by the specified portal
///
pub async fn get_usage_reports(
    me: &str,
    resource: &str,
    portal: &PortalIdentifier,
    dates: &DateRange,
) -> Result<UsageReport, Error> {
    // we need to connect to our bridge agent, so it can be used
    // to tell the connected portal software to get the reports.

    match agent::bridge(BRIDGE_WAIT_TIME).await {
        Some(bridge) => {
            // send the get_usage_report job to the bridge agent
            let job = Job::parse(
                &format!(
                    "{}.{}.{} get_usage_reports {} {}",
                    me,
                    bridge.name(),
                    resource,
                    portal,
                    dates
                ),
                false,
            )?
            .put(&bridge)
            .await?;

            // Wait for the job to complete
            let result = job.wait().await?.result::<UsageReport>()?;

            match result {
                Some(report) => {
                    tracing::debug!("Usage reports retrieved by bridge agent: {:?}", report);
                    Ok(report)
                }
                None => {
                    tracing::warn!("No usage reports retrieved?");
                    Err(Error::MissingProject(
                        "No usage reports retrieved by bridge agent".to_string(),
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
