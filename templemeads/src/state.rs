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
        Self {
            states: HashMap::new(),
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
        Self {
            board: Arc::new(RwLock::new(Board::new(&peer))),
        }
    }

    pub async fn board(&self) -> Arc<RwLock<Board>> {
        self.board.clone()
    }
}
