// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use crate::agent;
use crate::agent::{Peer, Type as AgentType};
use crate::command::Command;
use crate::control_message::process_control_message;
use crate::destination::Position;
use crate::diagnostics;
use crate::domain::Domain;
use crate::domain_static;
use crate::error::Error;
use crate::health;
use crate::job::{sync_from_peer, Envelope, Job, Status};
use crate::jobtiming;
use crate::notification::{default_notify_runner, AsyncNotifyRunnable, NotificationEnvelope};
use crate::portal_identifier::PortalIdentifier;
use crate::portalroutes;
use crate::restart;
use crate::runnable::{default_runner, AsyncRunnable};

use anyhow::Result;
use paddington::config::ServiceConfig;
use paddington::message::{Message, MessageType};
use std::any::Any;
use std::boxed::Box;
use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex, OnceLock};
use tokio::sync::RwLock;

#[derive(Debug, Clone)]
struct ServiceDetails<L: Domain> {
    service: String,
    agent_type: AgentType,
    runner: AsyncRunnable<L>,
    notify_runner: AsyncNotifyRunnable<L>,
    keepalives: Arc<Mutex<HashSet<String>>>,
    /// Whether this agent expects the Jobs it receives to be *portal-rooted* -
    /// i.e. for the first agent in a Job's destination to be the portal that
    /// owns the identifiers the instruction names. See
    /// `assert_portal_ownership` and
    /// `docs/specifications/security-review-2.md` (finding R34).
    verify_portal_ownership: bool,
}

impl<L: Domain> Default for ServiceDetails<L> {
    fn default() -> Self {
        ServiceDetails {
            service: String::new(),
            agent_type: agent::Type::Portal,
            runner: default_runner,
            notify_runner: default_notify_runner,
            keepalives: Arc::new(Mutex::new(HashSet::new())),
            // Off unless an agent opts in, so a new agent type cannot silently
            // acquire a check its destinations were never designed for.
            verify_portal_ownership: false,
        }
    }
}

static SERVICE_DETAILS: OnceLock<Box<dyn Any + Send + Sync>> = OnceLock::new();

fn service_details<L: Domain>() -> Result<&'static RwLock<ServiceDetails<L>>, Error> {
    domain_static::get_or_init(&SERVICE_DETAILS, || {
        RwLock::new(ServiceDetails::<L>::default())
    })
}

pub async fn set_my_service_details<L: Domain>(
    service: &str,
    agent_type: &agent::Type,
    runner: Option<AsyncRunnable<L>>,
    cascade_health: bool,
) -> Result<()> {
    let engine = env!("CARGO_PKG_NAME");
    let version = env!("CARGO_PKG_VERSION");

    tracing::info!("Agent layer: {} version {}", engine, version);

    agent::register_self(service, agent_type, engine, version, cascade_health).await;
    let mut service_details = service_details::<L>()?.write().await;
    service_details.service = service.to_string();
    service_details.agent_type = agent_type.clone();

    if let Some(runner) = runner {
        // only change this if a runner has been passed
        service_details.runner = runner;
    }

    Ok(())
}

/// Declare whether this agent expects portal-rooted Jobs (see
/// `ServiceDetails::verify_portal_ownership`).
///
/// Called by `provider::run`, `platform::run` and `instance::run`, which sit on
/// the portal-rooted path. `instance::run_delegated` sets it `false` for an
/// Instance whose Jobs are delegated by another agent rather than routed down
/// from the owning portal.
pub async fn set_verify_portal_ownership<L: Domain>(verify: bool) -> Result<()> {
    let mut service_details = service_details::<L>()?.write().await;
    service_details.verify_portal_ownership = verify;
    Ok(())
}

async fn verify_portal_ownership<L: Domain>() -> bool {
    match service_details::<L>() {
        Ok(details) => details.read().await.verify_portal_ownership,
        Err(_) => false,
    }
}

///
/// Push the portal routes we know to `peer`, if it is eligible to receive them.
///
/// A peer is eligible if it understands the message (advertised in its
/// `Register`) and is not itself declared a portal - routes travel away from
/// portals, never back toward them. `routes_for_peer` additionally withholds
/// anything we learned from that same peer.
///
async fn advertise_routes_to<L: Domain>(peer: &Peer) {
    if !agent::is_route_capable(peer).await {
        tracing::debug!(
            "Not advertising portal routes to {} - it does not support them",
            peer
        );
        return;
    }

    if agent::expected_peer_type(peer).await == Some(AgentType::Portal) {
        return;
    }

    let routes = portalroutes::routes_for_peer(peer).await;

    if routes.is_empty() {
        return;
    }

    tracing::debug!("Advertising {} portal route(s) to {}", routes.len(), peer);

    if let Err(e) = Command::<L>::portal_routes(&routes, &[])
        .send_to(peer)
        .await
    {
        tracing::warn!("Could not advertise portal routes to {}: {}", peer, e);
    }
}

/// Push our portal routes to every eligible peer, optionally skipping one
/// (normally the peer we just learned from, though `routes_for_peer` would
/// withhold its own routes anyway).
async fn advertise_routes_to_all<L: Domain>(skip: Option<&Peer>) {
    for peer in agent::all_peers().await {
        if Some(&peer) == skip {
            continue;
        }

        advertise_routes_to::<L>(&peer).await;
    }
}

///
/// Originate the routes for every peer our own config declares a portal, and
/// advertise them. Called once at startup, after the declared types are known.
///
/// This is the trust anchor: an agent adjacent to a portal is the only one that
/// asserts a portal's existence without having been told, and it does so purely
/// from its own configuration.
///
pub(crate) async fn originate_portal_routes<L: Domain>() {
    let me = agent::name().await;

    if me.is_empty() {
        return;
    }

    let mut originated = false;

    for (peer, typ) in agent::expected_peer_types().await {
        if typ != AgentType::Portal {
            continue;
        }

        let portal = match PortalIdentifier::parse(peer.name()) {
            Ok(portal) => portal,
            Err(e) => {
                tracing::error!(
                    "Peer {} is declared a portal but its name is not a valid portal \
                     identifier: {}",
                    peer,
                    e
                );
                continue;
            }
        };

        match portalroutes::originate(&portal, peer.zone(), &me).await {
            Ok(true) => originated = true,
            Ok(false) => {}
            Err(e) => tracing::error!("Could not originate a route for portal {}: {}", peer, e),
        }
    }

    if originated {
        advertise_routes_to_all::<L>(None).await;
    }
}

///
/// Tell our downstream peers that the routes we learned from `peer` are gone.
/// Called when that peer disconnects and its routes have been withdrawn.
///
pub(crate) async fn withdraw_routes_from<L: Domain>(peer: &Peer) {
    // The table has already dropped them, so re-advertising what remains is
    // enough for a peer that is still connected to converge - and the explicit
    // withdrawal below tells it which portals went away.
    advertise_routes_to_all::<L>(Some(peer)).await;
}

/// Re-check, on receipt, that a Job's instruction is being issued via the portal
/// that owns the identifiers it names.
///
/// `Command::parse`'s `check_portal` arm enforces this, but it is only ever
/// passed `true` at the two *entry* points to the system - where the bridge
/// parses a client command, and where the portal builds the southbound Job. Every
/// Job arriving over paddington is deserialised with `check_portal = false`
/// (`job.rs`'s `impl Deserialize for Command<L>`), and no privileged agent
/// re-checked it. So a Job injected directly at an agent inside the estate never
/// passed the check at all, and could name any portal's project while claiming
/// any first agent. See `docs/specifications/security-review-2.md` (finding
/// R34).
///
/// The decision is made entirely from locally-trusted state: this agent's own
/// declared expectation (from its own startup, not the wire) and
/// `Domain::owning_portal`, which lets `templemeads` ask the question without
/// knowing any domain vocabulary. An instruction that names no portal is not
/// checked.
async fn assert_portal_ownership<L: Domain>(job: &Job<L>, sender: &Peer) -> Result<(), Error> {
    let verify = verify_portal_ownership::<L>().await;

    // The root check (finding R34) - cheap, needs no protocol, and is the only
    // check available before any route has been learned.
    check_portal_ownership(job, verify)?;

    if !verify {
        return Ok(());
    }

    let Some(portal) = L::owning_portal(&job.instruction()) else {
        return Ok(());
    };

    assert_portal_route(job, &portal, sender).await
}

///
/// Check that a Job naming `portal` arrived by the route we expect that portal's
/// instructions to travel.
///
/// This is strictly stronger than the root check above, which compares only the
/// first agent of the destination: a correctly-named impostor portal introduced
/// one hop away satisfies the root check but produces a different route. See
/// `crate::portalroutes` and `docs/plans/portal-route-discovery-design.md`.
///
async fn assert_portal_route<L: Domain>(
    job: &Job<L>,
    portal: &PortalIdentifier,
    sender: &Peer,
) -> Result<(), Error> {
    let zone = sender.zone();
    let name = portal.portal();

    // Two conflicting routes have been advertised for this portal, so we cannot
    // tell which is genuine and refuse to route for it at all.
    if portalroutes::is_collided(zone, &name).await {
        tracing::error!(
            "Rejecting job {}: two conflicting routes have been advertised for portal '{}' \
             in zone '{}', so instructions naming it are refused until an operator resolves \
             it.",
            job.id(),
            name,
            zone
        );

        return Err(Error::InvalidInstruction(format!(
            "Portal '{}' has conflicting routes - refusing to route job {}",
            name,
            job.id()
        )));
    }

    let route = match portalroutes::expected_route(zone, &name).await {
        Some(route) => route,
        None => {
            // A peer that cannot send routes could never have told us one, so
            // holding their absence against it would break a mixed-version
            // fleet. This is the "absent means unchecked" rule R3 uses too.
            if !agent::is_route_capable(sender).await {
                return Ok(());
            }

            // Otherwise the route is expected but has not arrived yet. Wait for
            // it rather than rejecting: one task is spawned per inbound message,
            // so a Job can be processed before the route push delivered ahead of
            // it. This is not fail-open - nothing is accepted without a route.
            match portalroutes::wait_for_route(zone, &name).await {
                Some(route) => route,
                None => {
                    tracing::error!(
                        "Rejecting job {}: it names portal '{}' in zone '{}', but no route \
                         to that portal has been advertised to us.",
                        job.id(),
                        name,
                        zone
                    );

                    return Err(Error::InvalidInstruction(format!(
                        "No known route to portal '{}' - refusing job {}",
                        name,
                        job.id()
                    )));
                }
            }
        }
    };

    if !portalroutes::destination_matches_route(&job.destination(), &route) {
        tracing::error!(
            "Rejecting job {}: it names portal '{}' and arrived on destination '{}', but \
             that portal reaches us via '{}'. The route does not match, so this instruction \
             did not come from where it claims to have come from.",
            job.id(),
            name,
            job.destination(),
            route
        );

        return Err(Error::InvalidInstruction(format!(
            "Job {} names portal '{}' but arrived via '{}', not '{}'",
            job.id(),
            name,
            job.destination(),
            route
        )));
    }

    Ok(())
}

/// The decision `assert_portal_ownership` makes, with the policy passed in
/// rather than read from global state - so it can be tested directly.
fn check_portal_ownership<L: Domain>(job: &Job<L>, verify: bool) -> Result<(), Error> {
    if !verify {
        return Ok(());
    }

    let Some(portal) = L::owning_portal(&job.instruction()) else {
        return Ok(());
    };

    let first = job.destination().first();

    if portal.portal() != first {
        tracing::warn!(
            "Rejecting job {}: it names portal '{}' but arrived on a destination \
             rooted at '{}'. Only '{}' may issue instructions naming '{}'.",
            job.id(),
            portal.portal(),
            first,
            portal.portal(),
            portal.portal()
        );

        return Err(Error::InvalidInstruction(format!(
            "Job {} names portal '{}' but was issued via '{}'",
            job.id(),
            portal.portal(),
            first
        )));
    }

    Ok(())
}

pub async fn set_notify_runner<L: Domain>(runner: AsyncNotifyRunnable<L>) -> Result<()> {
    let mut service_details = service_details::<L>()?.write().await;
    service_details.notify_runner = runner;
    Ok(())
}

/// Deliver a notification directly to this agent's registered notify runner.
/// Used when the current agent is the final destination in the notification path.
pub async fn invoke_notify_runner<L: Domain>(envelope: NotificationEnvelope<L>) -> Result<()> {
    let runner = service_details::<L>()?.read().await.notify_runner;
    if let Err(e) = runner(envelope).await {
        tracing::warn!("Local notify runner returned error: {}", e);
    }
    Ok(())
}

///
/// This is the main function that processes a command sent via the OpenPortal system
/// This will either route the command to the right place, or if the command has reached
/// its destination it will take action
///
async fn process_command<L: Domain>(
    recipient: &str,
    sender: &str,
    zone: &str,
    command: &Command<L>,
    runner: &AsyncRunnable<L>,
    notify_runner: &AsyncNotifyRunnable<L>,
) -> Result<(), Error> {
    // Block new jobs during soft restart
    // Allow Register, HealthCheck, and Restart commands to pass through
    if paddington::is_soft_restart_in_progress() {
        match command {
            Command::Register { .. }
            | Command::HealthCheck { .. }
            | Command::Restart { .. }
            | Command::Notify { .. } => {
                // Allow these commands during soft restart
            }
            Command::Put { job } | Command::Update { job } => {
                // Error the job and send it back to the sender
                tracing::warn!(
                    "Rejecting job {} during soft restart from {}",
                    job.id(),
                    sender
                );

                let peer = Peer::new(sender, zone);
                let errored_job =
                    job.errored("Agent is performing a soft restart - please retry")?;

                // Send the errored job back to the sender
                if let Err(e) = errored_job.update(&peer).await {
                    tracing::warn!("Failed to send errored job back to sender: {}", e);
                }

                return Ok(());
            }
            _ => {
                // Reject other commands during soft restart
                tracing::warn!(
                    "Rejecting command during soft restart: {} from {}",
                    command,
                    sender
                );
                return Err(Error::Unavailable(
                    "Agent is currently performing a soft restart - please retry".to_string(),
                ));
            }
        }
    }

    match command {
        Command::Register {
            agent,
            engine,
            version,
            domain,
            domain_version,
            supports_portal_routes,
        } => {
            // A peer that didn't send a domain at all (pre-0.33.0) may still
            // be one this Domain recognises by historical version alone -
            // see `Domain::assume_legacy_domain_version`.
            let (domain, domain_version) = match (domain, domain_version) {
                (Some(d), Some(v)) => (Some(d.clone()), Some(v.clone())),
                _ => match L::assume_legacy_domain_version(version) {
                    Some(v) => (Some(L::name().to_string()), Some(v.to_string())),
                    None => (None, None),
                },
            };

            tracing::info!(
                "Registering agent: {}, engine={} version={} domain={} domain_version={}",
                agent,
                engine,
                version,
                domain.as_deref().unwrap_or("unknown"),
                domain_version.as_deref().unwrap_or("unknown")
            );

            // A peer's role arrives over the wire, and every type-based
            // authorization decision in the framework is made from it - which
            // portal accepts a `Submit`, which peer may restart us, which peer
            // an instance routes account operations to. Nothing checked it, so
            // any peer could claim any role. If our own config declares what
            // this peer should be, hold it to that. See
            // `docs/specifications/security-review-2.md` (finding R3).
            let sender_peer = Peer::new(sender, zone);

            if let Some(expected) = agent::expected_peer_type(&sender_peer).await {
                if *agent != expected {
                    tracing::error!(
                        "Refusing to register {}: it presents as agent type '{}', but our \
                         configuration declares it as '{}'. Ignoring this registration.",
                        sender_peer,
                        agent,
                        expected
                    );

                    return Err(Error::InvalidPeer(format!(
                        "Peer {} presented as '{}' but is declared as '{}'",
                        sender_peer, agent, expected
                    )));
                }
            } else {
                tracing::debug!(
                    "Peer {} has no declared agent type in our config, so its claimed \
                     type '{}' is accepted unchecked. Add `type = \"{}\"` to its config \
                     entry to have this verified.",
                    sender_peer,
                    agent,
                    agent
                );
            }

            agent::set_route_capable(&sender_peer, *supports_portal_routes).await;

            agent::register_peer(
                &Peer::new(sender, zone),
                agent,
                engine,
                version,
                domain.as_deref(),
                domain_version.as_deref(),
            )
            .await;

            // Now that the peer is registered - so we know its declared type
            // and whether it understands them - push it the portal routes we
            // know. This is the point at which the guarantee "a downstream
            // agent learns its route as soon as it connects" is established.
            advertise_routes_to::<L>(&sender_peer).await;
        }
        Command::Update { job } => {
            if job.is_expired() {
                tracing::debug!("Skipping expired job update: {}", job);
                return Ok(());
            }

            let peer = Peer::new(sender, zone);

            tracing::debug!("Update job: {:?} to {} from {}", job, recipient, peer,);

            assert_portal_ownership::<L>(job, &peer).await?;

            // update the sender's board with the received job
            let job = job.received(&peer).await?;

            // now see if we need to send this to the next agent
            match job.destination().position(recipient, sender) {
                Position::Upstream => {
                    // if we are upstream, then the job is moving backwards so we need to
                    // send it to the previous agent
                    if let Some(agent) = job.destination().previous(recipient) {
                        let peer = Peer::new(&agent, zone);
                        agent::wait_for(&peer, 30).await?;
                        job.update(&peer).await?;
                    }
                }
                Position::Downstream => {
                    // if we are downstream, then we continue to let the job
                    // flow downstream
                    if let Some(agent) = job.destination().next(recipient) {
                        let peer = Peer::new(&agent, zone);
                        agent::wait_for(&peer, 30).await?;
                        job.update(&peer).await?;
                    }
                }
                Position::Destination => {
                    tracing::debug!("Updated job has arrived at its destination: {}", job);
                }
                Position::Error => {
                    tracing::error!("Job has got into an errored position: {}", job);
                }
            }
        }
        Command::Put { job } => {
            if job.is_expired() {
                tracing::debug!("Skipping expired job put: {}", job);
                return Ok(());
            }

            let peer = Peer::new(sender, zone);

            tracing::debug!("Put job: {:?} to {} from {}", job, recipient, peer,);

            assert_portal_ownership::<L>(job, &peer).await?;

            // update the sender's board with the received job
            let mut job = match job.received(&peer).await {
                Ok(job) => job,
                Err(e) => {
                    tracing::error!("Error receiving job: {}", e);
                    let job = job.errored(&e.to_string())?;
                    let _ = job.update(&Peer::new(sender, zone)).await?;
                    return Ok(());
                }
            };

            // Keep a copy of the original job to detect if it changed
            let original_version = job.version();

            if job.is_duplicate() {
                tracing::debug!("Job is a duplicate for peer {}: {}", peer, job);

                // the existing job is being processed. We now need to wait
                // for that to finish - when it does, our new job will
                // be updated with the result
                while !job.is_finished() {
                    job = job.wait().await?;

                    if !job.is_finished() {
                        tracing::warn!("Still waiting for duplicate job to finish: {}", job);
                        job.assert_is_not_expired()?;
                    }
                }
            } else {
                match job.destination().position(recipient, sender) {
                    Position::Downstream => {
                        // if we are downstream, then we continue to let the job
                        // flow downstream
                        if let Some(agent) = job.destination().next(recipient) {
                            let peer = Peer::new(&agent, zone);

                            job = match job.put(&peer).await {
                                Ok(job) => job,
                                Err(e) => {
                                    tracing::error!("Error putting job: {}", e);
                                    job.errored(&e.to_string())?
                                }
                            }
                        }
                    }
                    Position::Destination => {
                        // we are the destination, so we need to take action
                        match job.state() {
                            Status::Complete => {
                                tracing::warn!(
                                    "Not rerunning job that has already completed: {}",
                                    job
                                );
                            }
                            Status::Error => {
                                tracing::warn!(
                                    "Not rerunning job that has already errored: {}",
                                    job
                                );
                            }
                            _ => {
                                tracing::info!(
                                    "Execute {} : {}",
                                    job.destination(),
                                    job.instruction()
                                );

                                // Start timing the job execution
                                let start_time = std::time::Instant::now();

                                // Record job started for diagnostics
                                diagnostics::record_job_started(&job).await;

                                job = match runner(Envelope::new(recipient, sender, zone, &job))
                                    .await
                                {
                                    Ok(job) => job,
                                    Err(e) => {
                                        tracing::error!("Error running job: {}", e);
                                        job.errored(&e.to_string())?
                                    }
                                };

                                // Record the job execution time
                                let duration = start_time.elapsed();
                                let duration_ms = duration.as_secs_f64() * 1000.0;
                                jobtiming::record_job_time(duration_ms);

                                // Record job finished for diagnostics
                                diagnostics::record_job_finished(&job).await;

                                // Track failures and slow jobs
                                if job.is_expired() {
                                    diagnostics::record_expired_job(&job).await;
                                } else if job.is_error() {
                                    let error_msg = job
                                        .error_message()
                                        .unwrap_or_else(|| "Unknown error".to_string());
                                    diagnostics::record_failed_job(&job, error_msg).await;
                                    diagnostics::record_slow_job(&job, duration_ms).await;
                                } else {
                                    diagnostics::record_completed_job(&job).await;
                                    diagnostics::record_slow_job(&job, duration_ms).await;
                                }

                                tracing::debug!(
                                    "Job {} completed in {:.2}ms",
                                    job.id(),
                                    duration_ms
                                );
                            }
                        }
                    }
                    Position::Error => {
                        tracing::error!("Job has got into an errored position: {}", job);
                        tracing::error!(
                            "Recipient: {}, Sender: {}, Destination: {}",
                            recipient,
                            sender,
                            job.destination()
                        );
                        job = job.errored("Job has got into an errored position")?;
                    }
                    _ => {
                        tracing::warn!("Job {} is being put, but is not moving?", job);
                        job = job.errored("Job has got into an unknown position")?;
                    }
                }
            }

            tracing::debug!("Job has finished: {}", job);

            // Only send updates if the job changed (version increased or state changed)
            // If we just forwarded it downstream without changes, the downstream agent will handle updates
            if job.version() == original_version {
                tracing::debug!(
                    "Job version unchanged ({}) - not sending update (job was forwarded or unchanged)",
                    original_version
                );
            } else {
                tracing::debug!(
                    "Job version changed ({} -> {}) - sending update",
                    original_version,
                    job.version()
                );
                // now the job has finished, update the sender's board
                // Check if the recipient is a virtual agent
                let recipient_peer = Peer::new(recipient, zone);

                if agent::is_self(&recipient_peer).await {
                    // Normal case: send update to the sender
                    tracing::debug!("Sending update of job {} back to sender {}", job, peer);
                    let _ = job.update(&peer).await?;
                } else if agent::is_virtual(&recipient_peer).await {
                    // Virtual agent case: use virtual_update
                    // recipient = virtual agent (e.g., isambard-ai)
                    // sender = hosting agent (e.g., waldur)
                    tracing::debug!(
                        "Sending virtual update of job {} to virtual agent {} via hosting agent {}",
                        job,
                        recipient_peer,
                        peer
                    );
                    let _ = job.virtual_update(&recipient_peer, &peer).await?;
                } else {
                    tracing::error!(
                        "Recipient {} is neither self nor virtual - not sending job update: {}",
                        recipient,
                        job
                    );
                }
            }
        }
        Command::Delete { job } => {
            if job.is_expired() {
                tracing::debug!("Skipping expired job delete: {}", job);
                return Ok(());
            }

            let peer = Peer::new(sender, zone);

            tracing::warn!("Delete job: {} to {} from {}", job, recipient, peer,);

            // record that the sender has deleted the job
            let job = job.deleted(&peer).await?;

            match job.destination().position(recipient, sender) {
                Position::Upstream => {
                    // if we are upstream, then the job is moving backwards so we need to
                    // send it to the previous agent
                    if let Some(agent) = job.destination().previous(recipient) {
                        let peer = Peer::new(&agent, zone);
                        agent::wait_for(&peer, 30).await?;
                        job.delete(&peer).await?;
                    }
                }
                Position::Downstream => {
                    // if we are downstream, then we continue to let the job
                    // flow downstream
                    if let Some(agent) = job.destination().next(recipient) {
                        let peer = Peer::new(&agent, zone);
                        agent::wait_for(&peer, 30).await?;
                        job.delete(&peer).await?;
                    }
                }
                Position::Error => {
                    tracing::error!("Job has got into an errored position: {}", job);
                }
                _ => {
                    tracing::warn!("Job {} is being deleted, but is not moving?", job);
                }
            }
        }
        Command::Sync { state } => {
            let peer = Peer::new(sender, zone);
            sync_from_peer(recipient, &peer, state).await?;
        }
        Command::HealthCheck { visited } => {
            tracing::debug!(
                "Received health check request from {} (visited chain: {:?})",
                sender,
                visited
            );

            // Security: Portals must not respond to health checks from other portals
            // to prevent information leakage between sites
            let my_type = agent::my_agent_type().await;
            let sender_peer = Peer::new(sender, zone);

            if my_type == agent::Type::Portal {
                if let Some(sender_type) = agent::agent_type(&sender_peer).await {
                    if sender_type == agent::Type::Portal {
                        tracing::warn!(
                            "Ignoring health check from portal {} - portals do not share health with other portals",
                            sender
                        );
                        return Ok(());
                    }
                }
            }

            // Collect health information (including cascaded peer health)
            let health = health::collect_health::<L>(sender, visited.clone()).await?;

            tracing::debug!("Health check: {}", health);

            // Send health response back to sender
            let response = Command::<L>::health_response(health);
            response.send_to(&sender_peer).await?;
        }
        Command::HealthResponse { health } => {
            tracing::debug!("Received health response: {}", health);
            // Cache the health response for later retrieval
            health::cache_health_response(*health.clone()).await;
        }
        Command::Restart {
            restart_type,
            destination,
        } => {
            restart::handle_restart_request::<L>(sender, restart_type, destination).await?;
        }
        Command::DiagnosticsRequest { destination } => {
            tracing::debug!(
                "Received diagnostics request from {} (destination: {})",
                sender,
                destination
            );

            // Security: Portals must not respond to diagnostics requests from other portals
            // to prevent information leakage between sites
            let my_type = agent::my_agent_type().await;
            let sender_peer = Peer::new(sender, zone);

            if my_type == agent::Type::Portal {
                if let Some(sender_type) = agent::agent_type(&sender_peer).await {
                    if sender_type == agent::Type::Portal {
                        tracing::warn!(
                            "Ignoring diagnostics request from portal {} - portals do not share diagnostics with other portals",
                            sender
                        );
                        return Ok(());
                    }
                }
            }

            // Collect diagnostics information (including cascaded peer diagnostics)
            let diagnostics_report = diagnostics::collect_diagnostics::<L>(destination).await?;

            tracing::debug!("Diagnostics report: {}", diagnostics_report);

            // Send diagnostics response back to sender
            let response = Command::<L>::diagnostics_response(diagnostics_report);
            response.send_to(&sender_peer).await?;
        }
        Command::PortalRoutes { routes, withdrawn } => {
            let peer = Peer::new(sender, zone);

            tracing::debug!(
                "Received {} portal route(s) and {} withdrawal(s) from {}",
                routes.len(),
                withdrawn.len(),
                peer
            );

            let me = agent::name().await;

            // If our own table changed, our downstream peers' routes changed
            // too, so pass it on. In an acyclic topology this terminates:
            // `routes_for_peer` never returns a route back to the peer it came
            // from, so nothing loops.
            if portalroutes::receive(&peer, routes, withdrawn, &me).await {
                advertise_routes_to_all::<L>(Some(&peer)).await;
            }
        }
        Command::DiagnosticsResponse { report } => {
            tracing::debug!("Received diagnostics response from {}", report.agent_name);

            // Cache the response using the agent_name as the key
            // This allows intermediate agents to retrieve it when forwarding responses
            diagnostics::cache_diagnostics_response(report.agent_name.clone(), *report.clone())
                .await;
        }
        Command::Notify { notification } => {
            diagnostics::increment_notification_received().await;
            tracing::debug!(
                "Notification [{}] from {}: {}",
                notification.id(),
                sender,
                notification.event()
            );

            match notification.destination().position(recipient, sender) {
                Position::Downstream => {
                    if let Some(next) = notification.destination().next(recipient) {
                        let next_peer = Peer::new(&next, zone);
                        let cmd = Command::notify(notification);
                        if let Err(e) = cmd.send_to(&next_peer).await {
                            tracing::warn!(
                                "Failed to forward notification [{}] to {}: {}",
                                notification.id(),
                                next_peer,
                                e
                            );
                        }
                    }
                }
                Position::Destination => {
                    tracing::debug!(
                        "Notification [{}] arrived at destination",
                        notification.id()
                    );
                    let envelope = NotificationEnvelope::new(recipient, sender, zone, notification);
                    if let Err(e) = notify_runner(envelope).await {
                        tracing::warn!("Error in notify runner for [{}]: {}", notification.id(), e);
                    }
                }
                Position::Error => {
                    // The recipient is not in the destination path. The only
                    // legitimate case is a bridge agent receiving a notification
                    // whose destination ends at (or one step past) the portal it
                    // is connected to — i.e. the portal is the last or penultimate
                    // agent in the path (penultimate covers virtual-agent suffixes).
                    let mut handled = false;

                    let my_agent_type = match service_details::<L>() {
                        Ok(service_details) => service_details.read().await.agent_type.clone(),
                        Err(e) => return Err(e),
                    };
                    if my_agent_type == AgentType::Bridge {
                        if let Some(portal) = agent::portal(0).await {
                            let agents = notification.destination().agents();
                            let n = agents.len();
                            let portal_is_last = agents.last().is_some_and(|a| *a == portal.name());
                            let portal_is_penultimate = n
                                .checked_sub(2)
                                .and_then(|i| agents.get(i))
                                .is_some_and(|a| *a == portal.name());

                            if portal_is_last || portal_is_penultimate {
                                tracing::debug!(
                                    "Notification [{}] accepted by bridge sidecar (portal={} is {} in path)",
                                    notification.id(),
                                    portal.name(),
                                    if portal_is_last { "last" } else { "penultimate" },
                                );
                                let envelope = NotificationEnvelope::new(
                                    recipient,
                                    sender,
                                    zone,
                                    notification,
                                );
                                if let Err(e) = notify_runner(envelope).await {
                                    tracing::warn!(
                                        "Error in notify runner for [{}]: {}",
                                        notification.id(),
                                        e
                                    );
                                }
                                handled = true;
                            }
                        }
                    }

                    if !handled {
                        tracing::warn!(
                            "Notification [{}] in errored position (recipient={}, sender={})",
                            notification.id(),
                            recipient,
                            sender
                        );
                    }
                }
                _ => {
                    tracing::warn!(
                        "Notification [{}] in unexpected position",
                        notification.id()
                    );
                }
            }
        }
        _ => {
            tracing::warn!("Command {} not recognised", command);
        }
    }

    Ok(())
}

///
/// Message handler for most templemeads agents
///
/// This is hand-expanded from paddington's `async_message_handler!` macro
/// (rather than using it directly) because that macro's pattern has no slot
/// for a generic parameter, and this function needs to be generic over the
/// chosen `Domain` - paddington itself stays untouched and domain-agnostic.
pub fn process_message<L: Domain>(
    message: Message,
) -> Pin<Box<dyn Future<Output = Result<(), paddington::Error>> + Send>> {
    Box::pin(async move {
        let service_info: ServiceDetails<L> = match service_details::<L>() {
            Ok(service_details) => service_details.read().await.to_owned(),
            Err(e) => return Err(paddington::Error::Any(e.into())),
        };

        match message.typ() {
            MessageType::Control => {
                process_control_message::<L>(&service_info.agent_type, message.into()).await?;
                Ok(())
            }
            MessageType::KeepAlive => {
                let sender: String = message.sender().to_owned();
                let recipient: String = message.recipient().to_owned();
                let zone: String = message.zone().to_owned();

                if recipient != service_info.service {
                    return Err(Error::Delivery(format!(
                        "Recipient {} does not match service {}",
                        recipient, service_info.service
                    ))
                    .into());
                }

                // check that we are the only one sending keepalives to this peer
                let name = format!("{}@{}", sender, zone);
                tracing::debug!("Keepalive message from {}", name);

                match service_info.keepalives.lock() {
                    Ok(mut keepalives) => {
                        if keepalives.contains(&name) {
                            tracing::debug!(
                                "Duplicate keepalive message from {} in zone {} - skipping",
                                sender,
                                zone
                            );
                            return Ok(());
                        }

                        keepalives.insert(name.clone());
                    }
                    Err(e) => {
                        tracing::warn!("Error locking keepalives: {}", e);
                        return Ok(());
                    }
                }

                // wait 23 seconds and send a keep alive message back
                tracing::debug!("Keepalive sleeping for 23 seconds from {}", name);
                tokio::time::sleep(tokio::time::Duration::from_secs(23)).await;
                tracing::debug!("Keepalive reawakened from {}", name);

                match service_info.keepalives.lock() {
                    Ok(mut keepalives) => {
                        keepalives.remove(&name);
                    }
                    Err(e) => {
                        tracing::error!("Error locking keepalives: {}", e);
                        return Ok(());
                    }
                }

                tracing::debug!("Sending keepalive message to {} again", name);
                match paddington::send(Message::keepalive(&sender, &zone)).await {
                    Ok(_) => {}
                    Err(e) => {
                        tracing::warn!("Error sending keepalive message to {} in zone {}: {}. Disconnecting peer.", sender, zone, e);
                        paddington::disconnect(&sender, &zone).await?;
                    }
                }

                tracing::debug!("End of keepalive for {}", name);

                Ok(())
            }
            MessageType::Message => {
                let sender: String = message.sender().to_owned();
                let recipient: String = message.recipient().to_owned();
                let zone: String = message.zone().to_owned();
                let command: Command<L> = message.into();

                if recipient != service_info.service {
                    // check to see if this is a virtual agent
                    if !agent::is_virtual(&Peer::new(&recipient, &zone)).await {
                        return Err(Error::Delivery(format!(
                            "Recipient {} does not match service {}",
                            recipient, service_info.service
                        ))
                        .into());
                    }
                }

                process_command(
                    &recipient,
                    &sender,
                    &zone,
                    &command,
                    &service_info.runner,
                    &service_info.notify_runner,
                )
                .await?;

                Ok(())
            }
        }
    })
}

///
/// Start the paddington event loop with blind relay support - every
/// `run()` in this crate calls this instead of calling
/// `paddington::set_handler`/`paddington::run` directly, so that any
/// `servers`/`clients` peer configured with a `proxy` (see
/// `paddington::relay` and `docs/plans/archive/blind-relay-proxy-design.md`) works
/// the same way for every agent kind without each one needing its own
/// wiring.
///
/// Registers [`process_message::<L>`] as the *inner* handler behind
/// [`paddington::relay::relay_dispatch_handler`] (which passes non-relay
/// traffic through unchanged, so this is a no-op for agents with no
/// relayed peers configured), then spawns bootstrapping of any relayed
/// peers this agent connects to as the relayed *client* - relayed
/// *servers* wait for their peer to initiate instead, so there's nothing
/// to spawn for them. Bootstrapping runs concurrently with, not before,
/// `paddington::run` below, since that's what actually dials the
/// underlying connection to the relay in the first place.
///
/// Read each peer's declared `type = "..."` out of the service config and hand
/// the result to the agent registrar, so `Command::Register` can check what a
/// peer claims against what we were told to expect.
///
/// A peer with no declared type is absent from the map and is not checked, so an
/// existing config keeps working and the check can be adopted per peer. An
/// unrecognised value is a misconfiguration: it is logged loudly and treated as
/// "not declared" rather than silently rejecting the peer. See
/// `docs/specifications/security-review-2.md` (finding R3).
async fn register_expected_peer_types(config: &ServiceConfig) {
    let mut expected: HashMap<Peer, AgentType> = HashMap::new();

    let declared = config
        .clients()
        .into_iter()
        .map(|c| (c.name(), c.zone(), c.agent_type()))
        .chain(
            config
                .servers()
                .into_iter()
                .map(|s| (s.name(), s.zone(), s.agent_type())),
        );

    for (name, zone, agent_type) in declared {
        let Some(agent_type) = agent_type else {
            continue;
        };

        match AgentType::parse(&agent_type) {
            Some(typ) => {
                expected.insert(Peer::new(&name, &zone), typ);
            }
            None => {
                tracing::error!(
                    "Peer {}@{} declares an unrecognised agent type '{}' - ignoring it, so \
                     this peer's claimed type will NOT be checked. Valid values are: \
                     portal, provider, platform, instance, bridge, account, filesystem, \
                     scheduler, virtual.",
                    name,
                    zone,
                    agent_type
                );
            }
        }
    }

    agent::set_expected_peer_types(expected).await;
}

pub async fn run_with_relay<L: Domain>(config: ServiceConfig) -> Result<(), paddington::Error> {
    register_expected_peer_types(&config).await;

    // Must follow the line above: origination reads the declared types to find
    // which of our peers are portals.
    originate_portal_routes::<L>().await;

    paddington::relay::configure(&config).await?;
    paddington::relay::set_inner_handler(process_message::<L>).await?;
    paddington::set_handler(paddington::relay::relay_dispatch_handler).await?;

    tokio::spawn(async {
        if let Err(e) = paddington::relay::bootstrap_all_as_client().await {
            tracing::error!("Could not bootstrap relayed peer(s): {:?}", e);
        }
    });

    paddington::run(config).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::job::Job;
    use crate::test_domain::TestDomain;

    fn job(command: &str) -> Job<TestDomain> {
        Job::parse(command, false).unwrap_or_else(|e| unreachable!("job: {:?}", e))
    }

    #[test]
    fn test_check_portal_ownership_rejects_a_foreign_portal_root() {
        // Regression test for finding R34. `Command::parse`'s `check_portal` arm
        // is only ever passed `true` at the two entry points to the system, and
        // every Job arriving over paddington is deserialised with it `false` -
        // so nothing re-checked that an instruction naming portal X had actually
        // been issued via X. A Job injected directly at an agent inside the
        // estate could therefore name any portal's project.

        // The legitimate shape: the instruction names `brics`, and the Job
        // arrived on a destination rooted at `brics`.
        assert!(
            check_portal_ownership(&job("brics.cluster add_user bob.proj.brics"), true).is_ok()
        );
        assert!(check_portal_ownership(
            &job("brics.provider.clusters.cluster add_user bob.proj.brics"),
            true
        )
        .is_ok());

        // The attack: the same instruction, arriving on a destination rooted at
        // something else. This is what an agent inside the estate could inject.
        assert!(check_portal_ownership(
            &job("attacker.clusters.cluster add_user bob.proj.brics"),
            true
        )
        .is_err());

        // ...including a route rooted at another real portal.
        assert!(
            check_portal_ownership(&job("otherportal.cluster add_user bob.proj.brics"), true)
                .is_err()
        );
    }

    #[test]
    fn test_check_portal_ownership_is_skipped_when_not_enabled() {
        // Account/Filesystem/Scheduler agents, and `instance::run_delegated`
        // Instances such as `op-cloudaccount`, receive Jobs whose destination is
        // rooted at the *delegating* agent rather than at the owning portal. For
        // them the property does not hold and must not be enforced.
        let delegated = job("cloudportal.cloudaccount add_user bob.proj.waldur");

        assert!(check_portal_ownership(&delegated, false).is_ok());
        // ...and it would indeed have been rejected had it been enabled, which
        // is why the distinction has to be declared rather than inferred.
        assert!(check_portal_ownership(&delegated, true).is_err());

        // The transformed instructions an instance sends its backends are the
        // same shape.
        let local = job("cluster.freeipa add_local_user bob.proj.brics");
        assert!(check_portal_ownership(&local, false).is_ok());
    }

    #[tokio::test]
    async fn test_assert_portal_route_enforces_the_discovered_route() {
        // End-to-end for the enforcement half of portal route discovery. Uses a
        // zone unique to this test, because the route table is a process-wide
        // singleton shared with every other test in this binary.
        use crate::portal_identifier::PortalIdentifier;
        use crate::portalroutes;

        let zone = "handler-route-enforcement-test";
        let brics =
            PortalIdentifier::parse("brics").unwrap_or_else(|e| unreachable!("portal: {:?}", e));

        // `clusters` learns from `aip1` that brics reaches it via
        // brics.aip1.clusters.
        let aip1 = Peer::new("aip1", zone);
        let advert = portalroutes::PortalRoute::new(
            &brics,
            &crate::destination::Destination::parse("brics.aip1")
                .unwrap_or_else(|e| unreachable!("dest: {:?}", e)),
        );
        assert!(portalroutes::receive(&aip1, &[advert], &[], "clusters").await);

        // A Job on the expected route is accepted.
        let good = job("brics.aip1.clusters add_user bob.proj.brics");
        assert!(assert_portal_route(&good, &brics, &aip1).await.is_ok());

        // The same Job with further hops beyond us is also fine - we may be an
        // intermediate.
        let onward = job("brics.aip1.clusters.shared add_user bob.proj.brics");
        assert!(assert_portal_route(&onward, &brics, &aip1).await.is_ok());

        // The attack this scheme exists for: a correctly-*named* impostor portal
        // introduced one hop away. The root check (R34) passes, because
        // `first()` really is `brics` - only the route reveals it.
        let impostor = job("brics.fake.clusters add_user bob.proj.brics");
        assert!(check_portal_ownership(&impostor, true).is_ok());
        assert!(assert_portal_route(&impostor, &brics, &aip1).await.is_err());

        // A route that is a prefix of, but shorter than, the expected one.
        let short = job("brics.clusters add_user bob.proj.brics");
        assert!(assert_portal_route(&short, &brics, &aip1).await.is_err());
    }

    #[tokio::test]
    async fn test_assert_portal_route_skips_a_peer_that_cannot_send_routes() {
        // Mixed-version fleet: a peer that never advertised support could not
        // have told us a route, so holding its absence against it would break
        // the rollout. Same "absent means unchecked" rule R3 uses.
        use crate::portal_identifier::PortalIdentifier;

        let zone = "handler-route-compat-test";
        let brics =
            PortalIdentifier::parse("brics").unwrap_or_else(|e| unreachable!("portal: {:?}", e));

        let old_peer = Peer::new("old-aip1", zone);
        agent::set_route_capable(&old_peer, false).await;

        // No route is known for this zone at all, and the peer is not capable -
        // so the Job passes.
        let job = job("brics.aip1.clusters add_user bob.proj.brics");
        assert!(assert_portal_route(&job, &brics, &old_peer).await.is_ok());
    }

    #[tokio::test]
    async fn test_assert_portal_route_refuses_a_collided_portal() {
        use crate::destination::Destination;
        use crate::portal_identifier::PortalIdentifier;
        use crate::portalroutes;

        let zone = "handler-route-collision-test";
        let brics =
            PortalIdentifier::parse("brics").unwrap_or_else(|e| unreachable!("portal: {:?}", e));

        let aip1 = Peer::new("aip1", zone);
        let fake = Peer::new("fake", zone);

        let route = |r: &str| {
            portalroutes::PortalRoute::new(
                &brics,
                &Destination::parse(r).unwrap_or_else(|e| unreachable!("dest: {:?}", e)),
            )
        };

        assert!(portalroutes::receive(&aip1, &[route("brics.aip1")], &[], "clusters").await);
        // The impostor's advertisement collides rather than replacing.
        assert!(!portalroutes::receive(&fake, &[route("brics.fake")], &[], "clusters").await);
        assert!(portalroutes::is_collided(zone, "brics").await);

        // With two conflicting routes we cannot tell which is genuine, so even a
        // Job on the originally-correct route is refused until an operator
        // resolves it.
        let good = job("brics.aip1.clusters add_user bob.proj.brics");
        assert!(assert_portal_route(&good, &brics, &aip1).await.is_err());
    }

    #[test]
    fn test_check_portal_ownership_ignores_instructions_naming_no_portal() {
        // An instruction that names no portal carries no ownership claim, so
        // there is nothing to check and it must pass either way.
        let no_portal = job("a.b something-with-no-identifier");

        assert!(check_portal_ownership(&no_portal, true).is_ok());
        assert!(check_portal_ownership(&no_portal, false).is_ok());
    }
}
