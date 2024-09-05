// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

// internal API
mod agent_bridge;
mod agent_core;
mod bridge_server;
mod command;

// public API
pub mod agent {
    pub mod bridge {
        pub use crate::agent_bridge::*;
    }

    pub mod portal {
        pub use crate::agent_core::process_args;
        pub use crate::agent_core::Config;
        pub use crate::agent_core::Defaults;
        pub use crate::agent_core::Type;
        pub use crate::portal::run;
    }

    pub use crate::agent_core::*;
}

pub mod board;
pub mod bridge;
pub mod job;
pub mod portal;

pub mod server {
    pub use crate::bridge_server::sign_api_call;
}
