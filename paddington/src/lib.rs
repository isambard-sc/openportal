// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

// internal API
mod anti_replay;
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
pub use crypto::{constant_time_eq, Key, SecretKey, Signature};
pub use error::Error;
pub use eventloop::run;
pub use exchange::disconnect;
pub use exchange::is_soft_restart_in_progress;
pub use exchange::received;
pub use exchange::send;
pub use exchange::set_handler;
pub use exchange::watchdog;
pub use exchange::worker_count;
pub use exchange::SoftRestartGuard;
pub mod invite;
pub mod message;
pub mod relay;
