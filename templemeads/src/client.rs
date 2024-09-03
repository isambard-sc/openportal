// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use anyhow::Error as AnyError;
use thiserror::Error;

use crate::board::Handle;

pub fn add_user_to_instance_in_project(
    user: &str,
    instance: &str,
    project: &str,
) -> Result<Handle, Error> {
    tracing::info!(
        "Adding user {} to instance {} in project {}",
        user,
        instance,
        project
    );
    Ok(Handle {
        id: "123".to_string(),
    })
}

pub fn add_user_to_project(user: &str, project: &str) -> Result<Handle, Error> {
    tracing::info!("Adding user {} to project {}", user, project);
    Ok(Handle {
        id: "456".to_string(),
    })
}

/// Errors

#[derive(Error, Debug)]
pub enum Error {
    #[error("{0}")]
    AnyError(#[from] AnyError),

    #[error("Unknown error")]
    Unknown,
}
