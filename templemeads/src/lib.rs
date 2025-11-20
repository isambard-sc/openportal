// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

// internal API
mod account;
mod agent_bridge;
mod agent_core;
mod bridge_server;
mod bridgeboard;
mod bridgestate;
mod control_message;
mod custom;
mod diagnostics;
mod error;
mod filesystem;
mod handler;
mod instance;
mod jobtiming;
mod platform;
mod portal;
mod provider;
mod restart;
mod scheduler;
mod systeminfo;
mod virtual_agent;

// public API
pub mod agent;
pub mod board;
pub mod bridge;
pub mod command;
pub mod config;
pub mod destination;
pub use error::Error;
pub mod grammar;
pub mod health;
pub mod job;
pub mod runnable;
pub mod state;
pub mod usagereport;

pub mod server {
    pub use crate::bridge_server::sign_api_call;
    pub use crate::bridgestate::get as get_board;
}

// Re-export system info monitor for agents to use at startup
pub use systeminfo::spawn_monitor as spawn_system_monitor;
