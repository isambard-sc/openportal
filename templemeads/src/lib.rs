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
mod error;
mod filesystem;
mod handler;
mod instance;
mod jobtiming;
mod notificationstate;
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
pub mod diagnostics;
pub mod domain;
mod domain_static;
pub use error::Error;
pub mod health;
pub mod job;
pub mod named;
pub mod notification;
pub mod portal_identifier;
pub mod runnable;
pub mod state;
#[cfg(test)]
mod test_domain;

pub mod server {
    pub use crate::bridge_server::sign_api_call;
    pub use crate::bridgestate::get as get_board;
    pub use crate::notificationstate::add as add_pending_notification;
    pub use crate::notificationstate::enqueue as enqueue_notification;
    pub use crate::notificationstate::get as get_pending_notification;
    pub use crate::notificationstate::pop_queued as pop_queued_notification;
    pub use crate::notificationstate::remove as remove_pending_notification;
}

// Re-export system info monitor for agents to use at startup
pub use systeminfo::spawn_monitor as spawn_system_monitor;

// Re-export notification runner setter and local invoker
pub use handler::invoke_notify_runner;
pub use handler::set_notify_runner;

#[cfg(test)]
mod tests {
    use crate::agent::Type as AgentType;
    use crate::diagnostics::{
        DiagnosticsReport, ExpiredJobEntry, FailedJobEntry, JobStatistics, LogEntry,
        RunningJobEntry, SlowJobEntry,
    };
    use crate::health::HealthInfo;
    use crate::job::Status;
    use ts_rs::TS;

    // Domain-vocabulary types (Instruction/identifiers, usage/storage
    // reports, Link/Note/MembershipControl/AwardDetails, ...) export their
    // own TS bindings from the domain crate (e.g. greatwestern) that owns
    // them - templemeads only exports the framework-level types below,
    // which exist regardless of which Domain an agent is built against.
    #[allow(clippy::expect_used)]
    #[test]
    fn export_ts_bindings() {
        AgentType::export_all().expect("Could not export AgentType");
        Status::export_all().expect("Could not export Status");
        JobStatistics::export_all().expect("Could not export JobStatistics");
        DiagnosticsReport::export_all().expect("Could not export DiagnosticsReport");
        FailedJobEntry::export_all().expect("Could not export FailedJobEntry");
        SlowJobEntry::export_all().expect("Could not export SlowJobEntry");
        ExpiredJobEntry::export_all().expect("Could not export ExpiredJobEntry");
        RunningJobEntry::export_all().expect("Could not export RunningJobEntry");
        LogEntry::export_all().expect("Could not export LogEntry");
        HealthInfo::export_all().expect("Could not export HealthInfo");
    }
}
