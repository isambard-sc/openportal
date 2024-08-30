// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use anyhow::Error as AnyError;
use thiserror::Error;

pub struct Provider {
    pub id: String,
    pub name: String,
}

impl Provider {
    pub async fn find_by_instance(instance: &String) -> Result<Provider, Error> {
        Ok(Provider {
            id: "1234567890".to_string(),
            name: "provider".to_string(),
        })
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
