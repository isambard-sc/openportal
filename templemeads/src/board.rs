// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use anyhow::Error as AnyError;
use once_cell::sync::Lazy;
use std::collections::HashMap;
use std::sync::RwLock;
use thiserror::Error;

use crate::job::Job;

#[derive(Debug, Clone, Hash, Eq, PartialEq)]
pub struct Handle {
    pub id: String,
}

// We use the singleton pattern for the board data, as there can only
// be one in the program, and this will let us expose the board functions
// directly
static SINGLETON_BOARD: Lazy<RwLock<Board>> = Lazy::new(|| RwLock::new(Board::new()));

///
/// Each agent has a single board that holds all of the jobs that it is
/// responsible for.
///
pub struct Board {
    jobs: HashMap<Handle, Job>,
}

pub async fn submit(job: Job) -> Result<Handle, Error> {
    let mut board = SINGLETON_BOARD.write().unwrap();
    let handle = Handle {
        id: "123".to_string(),
    };
    board.jobs.insert(handle.clone(), job);
    Ok(handle)
}

impl Board {
    pub fn new() -> Self {
        Self {
            jobs: HashMap::new(),
        }
    }
}

/// Errors

#[derive(Error, Debug)]
pub enum Error {
    #[error("{0}")]
    AnyError(#[from] AnyError),

    #[error("Unknown error")]
    Unknown,
}
