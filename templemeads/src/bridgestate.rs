// SPDX-FileCopyrightText: © 2025 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use crate::bridgeboard::BridgeBoard;
use crate::error::Error;

use anyhow::Result;
use once_cell::sync::Lazy;
use std::sync::Arc;
use tokio::sync::RwLock;

struct State {
    board: Arc<RwLock<BridgeBoard>>,
}

static STATE: Lazy<RwLock<State>> = Lazy::new(|| RwLock::new(State::new()));

impl State {
    fn new() -> Self {
        start_cleaner();

        Self {
            board: Arc::new(RwLock::new(BridgeBoard::new())),
        }
    }
}

///
/// Return the board for the bridge
///
pub async fn get() -> Result<Arc<RwLock<BridgeBoard>>, Error> {
    let state = STATE.read().await;
    Ok(state.board.clone())
}

///
/// Function called in a tokio task to clean up the board
///
fn start_cleaner() {
    tokio::spawn(async {
        loop {
            tokio::time::sleep(tokio::time::Duration::from_secs(60)).await;
            clean_board().await;
        }
    });
}

///
/// Call this function to clean up the expired jobs from the board
///
async fn clean_board() {
    let state = match get().await {
        Ok(state) => state,
        Err(e) => {
            tracing::error!("Error getting state: {}", e);
            return;
        }
    };

    state.write().await.remove_expired_jobs();
}
