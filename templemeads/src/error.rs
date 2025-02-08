// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use anyhow::Error as AnyError;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum Error {
    #[error("{0}")]
    AddrParse(#[from] std::net::AddrParseError),

    #[error("{0}")]
    Any(#[from] AnyError),

    #[error("{0}")]
    IO(#[from] std::io::Error),

    #[error("{0}")]
    Paddington(#[from] paddington::Error),

    #[error("{0}")]
    SerdeJson(#[from] serde_json::Error),

    #[error("{0}")]
    UrlParse(#[from] url::ParseError),

    #[error("{0}")]
    Bug(String),

    #[error("{0}")]
    Call(String),

    #[error("{0}")]
    ConfigExists(String),

    #[error("{0}")]
    Delivery(String),

    #[error("{0}")]
    IncompleteCode(String),

    #[error("{0}")]
    InvalidBoard(String),

    #[error("{0}")]
    InvalidConfig(String),

    #[error("{0}")]
    InvalidInstruction(String),

    #[error("{0}")]
    InvalidPeer(String),

    #[error("{0}")]
    InvalidState(String),

    #[error("{0}")]
    Duplicate(String),

    #[error("{0}")]
    Expired(String),

    #[error("{0}")]
    Locked(String),

    #[error("{0}")]
    Login(String),

    #[error("{0}")]
    Misconfigured(String),

    #[error("{0}")]
    MissingAgent(String),

    #[error("{0}")]
    MissingProject(String),

    #[error("{0}")]
    MissingUser(String),

    #[error("{0}")]
    NoPortal(String),

    #[error("{0}")]
    InvalidPortal(String),

    #[error("{0}")]
    NotFound(String),

    #[error("{0}")]
    Parse(String),

    #[error("{0}")]
    PeerEdit(String),

    #[error("{0}")]
    Run(String),

    #[error("{0}")]
    State(String),

    #[error("{0}")]
    Unknown(String),

    #[error("{0}")]
    UnknownInstruction(String),

    #[error("{0}")]
    UnmanagedUser(String),

    #[error("{0}")]
    UnmanagedGroup(String),
}

// implement into a paddington::Error
impl From<Error> for paddington::Error {
    fn from(e: Error) -> paddington::Error {
        paddington::Error::Derived(format!("{}", e))
    }
}
