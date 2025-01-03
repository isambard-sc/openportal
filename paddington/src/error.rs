// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use anyhow::Error as AnyError;
use std::io::Error as IOError;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum Error {
    #[error("{0}")]
    IO(#[from] IOError),

    #[error("{0}")]
    Any(#[from] AnyError),

    #[error("{0}")]
    Tungstenite(#[from] tokio_tungstenite::tungstenite::error::Error),

    #[error("{0}")]
    TokioTask(#[from] tokio::task::JoinError),

    #[error("{0}")]
    BusyLine(String),

    #[error("{0}")]
    Derived(String),

    #[error("{0}")]
    InvalidPeer(String),

    #[error("{0}")]
    NotExists(String),

    #[error("{0}")]
    Null(String),

    #[error("{0}")]
    Parse(String),

    #[error("{0}")]
    Peer(String),

    #[error("{0}")]
    Poison(String),

    #[error("{0}")]
    Send(String),

    #[error("{0}")]
    UnknownPeer(String),

    #[error("{0}")]
    UnnamedConnection(String),

    #[error("{0}")]
    Incompatible(String),
}
