// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use crate::agent::Peer;
use crate::board::Board;
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
        board.write().await.remove_expired_jobs();
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
