// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use std::io::Error as IOError;
use std::path;

#[derive(Debug)]
pub struct Config {
    pub server: String,
    pub port: u16,
}

pub fn create(config_dir: &path::Path, service_name: &String) -> Result<Config, IOError> {
    // see if this config_dir exists - return an error if it does
    if config_dir.exists() {
        return Err(IOError::new(
            std::io::ErrorKind::AlreadyExists,
            "Config directory already exists",
        ));
    }

    // create the config directory
    std::fs::create_dir_all(config_dir)?;

    // read the config and return it
    load(config_dir)
}

pub fn load(config_dir: &path::Path) -> Result<Config, IOError> {
    // see if this config_dir exists - return an error if it doesn't
    if !config_dir.exists() {
        return Err(IOError::new(
            std::io::ErrorKind::NotFound,
            "Config directory not found",
        ));
    }

    let config = Config {
        server: "localhost".to_string(),
        port: 8080,
    };
    Ok(config)
}
