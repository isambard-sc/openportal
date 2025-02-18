// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

// internal API
mod client;
mod connection;
mod crypto;
mod error;
mod eventloop;
mod exchange;
mod healthcheck;
mod server;

// public API
pub mod command;
pub mod config;
pub use crypto::{Key, SecretKey, Signature};
pub use error::Error;
pub use eventloop::run;
pub use exchange::disconnect;
pub use exchange::received;
pub use exchange::send;
pub use exchange::set_handler;
pub use exchange::watchdog;
pub mod invite;
pub mod message;
