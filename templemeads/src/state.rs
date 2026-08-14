// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use crate::agent;
use crate::board;
use crate::command::Command as ControlCommand;
use crate::destination;
use crate::domain::Domain;
use crate::domain_static;
use crate::error::Error;

/// Maximum number of per-peer boards this process will hold.
///
/// Boards are created on demand from the peer name in a Job's destination and then
/// never removed, so this is what stops a peer from leaving behind one board per
/// invented name. A real agent has a handful of peers; this is far above that while
/// still being a bound. See `docs/specifications/security-review-2.md` (finding R31).
const MAX_BOARDS: usize = 1_000;

use anyhow::Result;
use std::any::Any;
use std::collections::HashMap;
use std::sync::{Arc, OnceLock};
use tokio::sync::RwLock;

struct States<L: Domain> {
    states: HashMap<agent::Peer, Arc<State<L>>>,
}

static STATES: OnceLock<Box<dyn Any + Send + Sync>> = OnceLock::new();

fn states<L: Domain>() -> Result<&'static RwLock<States<L>>, Error> {
    domain_static::get_or_init(&STATES, || {
        start_cleaner::<L>();
        RwLock::new(States::<L>::new())
    })
}

impl<L: Domain> States<L> {
    fn new() -> Self {
        Self {
            states: HashMap::new(),
        }
    }
}

///
/// Function called in a tokio task to clean up the boards
///
fn start_cleaner<L: Domain>() {
    tokio::spawn(async {
        loop {
            tokio::time::sleep(tokio::time::Duration::from_secs(60)).await;
            clean_boards::<L>().await;
        }
    });
}

///
/// Call this function to clean up the expired jobs from the boards
///
async fn clean_boards<L: Domain>() {
    // Get our own peer identity
    let my_peer = agent::get_self(None).await;
    let my_name = my_peer.name();

    let peers = match states::<L>() {
        Ok(states) => states
            .read()
            .await
            .states
            .keys()
            .cloned()
            .collect::<Vec<agent::Peer>>(),
        Err(e) => {
            tracing::error!("Error getting states: {}", e);
            return;
        }
    };

    for peer in peers.iter() {
        let state = match get::<L>(peer).await {
            Ok(state) => state,
            Err(e) => {
                tracing::error!("Error getting state for {}: {}", peer, e);
                continue;
            }
        };

        let board = state.board().await;
        let expired_jobs = board.write().await.remove_expired_jobs();

        // For each expired job, check if we need to send it back upstream
        for job in expired_jobs {
            // Determine our position in the job's destination path
            // If we are downstream (received this job from upstream), send error back
            let position = job.destination().position(my_name, peer.name());

            match position {
                destination::Position::Downstream | destination::Position::Destination => {
                    // We received this job from upstream (via peer), but it expired
                    // We need to send the error back to the previous hop in the path
                    // The job's destination tells us where it was going, and peer is who we tried to send it to
                    // We need to find who sent it to us by looking at the destination path

                    // Get the previous agent in the destination path (who sent it to us)
                    if let Some(previous_agent) = job.destination().previous(my_name) {
                        // the zone must be the same as the board peer's zone, as
                        // all communication is zone-specific
                        let previous_peer = agent::Peer::new(&previous_agent, peer.zone());

                        tracing::debug!(
                            "Sending expired job error back to upstream peer: {}",
                            previous_peer
                        );

                        match ControlCommand::update(&job).send_to(&previous_peer).await {
                            Ok(_) => {}
                            Err(e) => {
                                tracing::error!(
                                    "Failed to send expired job update to {}: {}",
                                    previous_peer,
                                    e
                                );
                            }
                        }
                    } else {
                        // No previous agent - we originated this job
                        tracing::debug!(
                            "Expired job {} was originated by us, no upstream to notify",
                            job.id()
                        );
                    }
                }
                destination::Position::Upstream => {
                    // We are sending this job back upstream (it's a response)
                    // Send the error to the peer this job was meant for
                    tracing::debug!("No need to send expiry message upstream to: {}", peer);
                }
                destination::Position::Error => {
                    tracing::debug!(
                        "Expired job {} has invalid destination position, cannot send error back",
                        job.id()
                    );
                }
            }
        }
    }
}

async fn _force_get<L: Domain>(peer: agent::Peer) -> Result<Arc<State<L>>, Error> {
    let states = states::<L>()?;
    let mut states = states.write().await;

    // A `State` (and its `Board`) is created on demand for whatever peer name a
    // Job's destination happens to carry, and is then kept for the life of the
    // process. Nothing bounded that, so an attacker-chosen sequence of names each
    // left a permanent board behind. Real deployments have a handful of peers. See
    // `docs/specifications/security-review-2.md` (finding R31).
    if !states.states.contains_key(&peer) && states.states.len() >= MAX_BOARDS {
        tracing::warn!(
            "Refusing to create a board for '{}': {} boards already exist (the \
             limit). Either this agent has far more peers than expected, or a peer \
             is sending jobs naming agents that do not exist.",
            peer,
            states.states.len()
        );

        return Err(Error::Unavailable(format!(
            "Cannot create a board for '{}' - the {} board limit has been reached",
            peer, MAX_BOARDS
        )));
    }

    Ok(states
        .states
        .entry(peer.clone())
        .or_insert(Arc::new(State::new(peer.clone())))
        .clone())
}

async fn _get<L: Domain>(peer: &agent::Peer) -> Result<Option<Arc<State<L>>>, Error> {
    Ok(states::<L>()?.read().await.states.get(peer).cloned())
}

pub async fn get<L: Domain>(peer: &agent::Peer) -> Result<Arc<State<L>>, Error> {
    if let Some(state) = _get::<L>(peer).await? {
        Ok(state)
    } else {
        Ok(_force_get::<L>(peer.clone()).await?)
    }
}

#[derive(Debug, Clone)]
pub struct State<L: Domain> {
    board: Arc<RwLock<board::Board<L>>>,
}

impl<L: Domain> State<L> {
    pub fn new(peer: agent::Peer) -> Self {
        tracing::debug!("Creating new board for agent {}", peer);

        Self {
            board: Arc::new(RwLock::new(board::Board::new(&peer))),
        }
    }

    pub async fn board(&self) -> Arc<RwLock<board::Board<L>>> {
        self.board.clone()
    }
}

///
/// Collect aggregate job statistics from all boards
///
pub async fn aggregate_job_stats<L: Domain>() -> Result<board::BoardJobStats, Error> {
    let my_name = agent::name().await;
    let states = states::<L>()?.read().await;
    let mut totals = board::BoardJobStats::default();

    for state in states.states.values() {
        let board = state.board().await;
        let board = board.read().await;
        let stats = board.job_stats(&my_name);
        totals.active += stats.active;
        totals.pending += stats.pending;
        totals.running += stats.running;
        totals.completed += stats.completed;
        totals.duplicates += stats.duplicates;
        totals.successful += stats.successful;
        totals.expired += stats.expired;
        totals.errored += stats.errored;
        totals.in_flight += stats.in_flight;
        totals.queued += stats.queued;
    }

    Ok(totals)
}
