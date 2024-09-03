// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

// internal API
mod args;
mod client;
mod config;
mod connection;
mod crypto;
mod eventloop;
mod exchange;
mod server;

// public API
pub use args::Defaults;
pub use crypto::{Error as CryptoError, Key, SecretKey, Signature};
pub use eventloop::run;
pub use exchange::send;
pub use exchange::set_handler;
pub use exchange::Error;
pub use exchange::Message;
