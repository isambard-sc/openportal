// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use crate::board::Board;
use crate::error::Error;

use anyhow::Result;
use once_cell::sync::Lazy;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

struct States {
    states: HashMap<String, Arc<State>>,
}

static STATES: Lazy<RwLock<States>> = Lazy::new(|| RwLock::new(States::new()));

impl States {
    fn new() -> Self {
        Self {
            states: HashMap::new(),
        }
    }
}

async fn _force_get(key: &str) -> Result<Arc<State>, Error> {
    Ok(STATES
        .write()
        .await
        .states
        .entry(key.to_string())
        .or_insert(Arc::new(State::new(key)))
        .clone())
}

async fn _get(key: &str) -> Result<Option<Arc<State>>, Error> {
    Ok(STATES.read().await.states.get(key).cloned())
}

pub async fn get(key: &str) -> Result<Arc<State>, Error> {
    if let Some(state) = _get(key).await? {
        Ok(state)
    } else {
        Ok(_force_get(key).await?)
    }
}

#[derive(Debug, Default, Clone)]
pub struct State {
    board: Arc<RwLock<Board>>,
}

impl State {
    pub fn new(key: &str) -> Self {
        Self {
            board: Arc::new(RwLock::new(Board::new(key))),
        }
    }

    pub async fn board(&self) -> Arc<RwLock<Board>> {
        self.board.clone()
    }
}
