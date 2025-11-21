// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use crate::agent::Peer;
use crate::board::Board;
use crate::command::Command as ControlCommand;
use crate::error::Error;

use anyhow::Result;
use once_cell::sync::Lazy;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

struct States {
    states: HashMap<Peer, Arc<State>>,
}

static STATES: Lazy<RwLock<States>> = Lazy::new(|| RwLock::new(States::new()));

impl States {
    fn new() -> Self {
        start_cleaner();

        Self {
            states: HashMap::new(),
        }
    }
}

///
/// Function called in a tokio task to clean up the boards
///
fn start_cleaner() {
    tokio::spawn(async {
        loop {
            tokio::time::sleep(tokio::time::Duration::from_secs(60)).await;
            clean_boards().await;
        }
    });
}

///
/// Call this function to clean up the expired jobs from the boards
///
async fn clean_boards() {
    use crate::agent;
    use crate::destination::Position;

    // Get our own peer identity
    let my_peer = agent::get_self(None).await;
    let my_name = my_peer.name();

    let peers = STATES
        .read()
        .await
        .states
        .keys()
        .cloned()
        .collect::<Vec<Peer>>();

    for peer in peers.iter() {
        let state = match get(peer).await {
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
                Position::Downstream | Position::Destination => {
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
                Position::Upstream => {
                    // We are sending this job back upstream (it's a response)
                    // Send the error to the peer this job was meant for
                    tracing::debug!("No need to send expiry message upstream to: {}", peer);
                }
                Position::Error => {
                    tracing::warn!(
                        "Expired job {} has invalid destination position, cannot send error back",
                        job.id()
                    );
                }
            }
        }
    }
}

async fn _force_get(peer: Peer) -> Result<Arc<State>, Error> {
    Ok(STATES
        .write()
        .await
        .states
        .entry(peer.clone())
        .or_insert(Arc::new(State::new(peer.clone())))
        .clone())
}

async fn _get(peer: &Peer) -> Result<Option<Arc<State>>, Error> {
    Ok(STATES.read().await.states.get(peer).cloned())
}

pub async fn get(peer: &Peer) -> Result<Arc<State>, Error> {
    if let Some(state) = _get(peer).await? {
        Ok(state)
    } else {
        Ok(_force_get(peer.clone()).await?)
    }
}

#[derive(Debug, Default, Clone)]
pub struct State {
    board: Arc<RwLock<Board>>,
}

impl State {
    pub fn new(peer: Peer) -> Self {
        tracing::debug!("Creating new board for agent {}", peer);

        Self {
            board: Arc::new(RwLock::new(Board::new(&peer))),
        }
    }

    pub async fn board(&self) -> Arc<RwLock<Board>> {
        self.board.clone()
    }
}

///
/// Collect aggregate job statistics from all boards
/// Returns (active, pending, running, completed, duplicates, successful, expired, errored)
///
pub async fn aggregate_job_stats() -> (usize, usize, usize, usize, usize, usize, usize, usize) {
    let states = STATES.read().await;
    let mut total_active = 0;
    let mut total_pending = 0;
    let mut total_running = 0;
    let mut total_completed = 0;
    let mut total_duplicates = 0;
    let mut total_successful = 0;
    let mut total_expired = 0;
    let mut total_errored = 0;

    for state in states.states.values() {
        let board = state.board().await;
        let board = board.read().await;
        let (active, pending, running, completed, duplicates, successful, expired, errored) =
            board.job_stats();
        total_active += active;
        total_pending += pending;
        total_running += running;
        total_completed += completed;
        total_duplicates += duplicates;
        total_successful += successful;
        total_expired += expired;
        total_errored += errored;
    }

    (
        total_active,
        total_pending,
        total_running,
        total_completed,
        total_duplicates,
        total_successful,
        total_expired,
        total_errored,
    )
}
