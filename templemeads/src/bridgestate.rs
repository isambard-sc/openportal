// SPDX-FileCopyrightText: © 2025 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use crate::bridgeboard::BridgeBoard;
use crate::domain::Domain;
use crate::domain_static;
use crate::error::Error;

use anyhow::Result;
use std::any::Any;
use std::sync::{Arc, OnceLock};
use tokio::sync::RwLock;

struct State<L: Domain> {
    board: Arc<RwLock<BridgeBoard<L>>>,
}

static STATE: OnceLock<Box<dyn Any + Send + Sync>> = OnceLock::new();

fn state<L: Domain>() -> Result<&'static RwLock<State<L>>, Error> {
    domain_static::get_or_init(&STATE, || {
        start_cleaner::<L>();
        RwLock::new(State::new())
    })
}

impl<L: Domain> State<L> {
    fn new() -> Self {
        Self {
            board: Arc::new(RwLock::new(BridgeBoard::new())),
        }
    }
}

///
/// Return the board for the bridge
///
pub async fn get<L: Domain>() -> Result<Arc<RwLock<BridgeBoard<L>>>, Error> {
    let state = state::<L>()?.read().await;
    Ok(state.board.clone())
}

///
/// Function called in a tokio task to clean up the board
///
fn start_cleaner<L: Domain>() {
    tokio::spawn(async {
        loop {
            tokio::time::sleep(tokio::time::Duration::from_secs(60)).await;
            clean_board::<L>().await;
        }
    });
}

///
/// Call this function to clean up the expired jobs from the board
///
async fn clean_board<L: Domain>() {
    let state = match get::<L>().await {
        Ok(state) => state,
        Err(e) => {
            tracing::error!("Error getting state: {}", e);
            return;
        }
    };

    state.write().await.remove_expired_jobs();
}
