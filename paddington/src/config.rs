// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use orion::aead;
use serde::{Deserialize, Serialize};
use serde_json;
use std::io::Error as IOError;
use std::path;

#[derive(Serialize, Deserialize, Debug)]
pub struct ServiceConfig {
    pub name: String,
    pub server: String,
    pub port: u16,
}

pub fn create(
    config_dir: &path::Path,
    service_name: &Option<String>,
    server: &Option<String>,
    port: &Option<u16>,
) -> Result<ServiceConfig, IOError> {
    // see if this config_dir exists - return an error if it does
    if config_dir.exists() {
        return Err(IOError::new(
            std::io::ErrorKind::AlreadyExists,
            "Config directory already exists",
        ));
    }

    // create the config directory
    std::fs::create_dir_all(config_dir)?;

    let service_name = service_name.clone().unwrap_or("openportal".to_string());
    let server = server.clone().unwrap_or("localhost".to_string());
    let port = port.clone().unwrap_or(8080);

    let config = ServiceConfig {
        name: service_name.clone(),
        //service_key: aead::SecretKey::default(),
        server: server.clone(),
        port,
    };

    // read the config and return it
    load(config_dir)
}

pub fn load(config_dir: &path::Path) -> Result<ServiceConfig, IOError> {
    // see if this config_dir exists - return an error if it doesn't
    if !config_dir.exists() {
        return Err(IOError::new(
            std::io::ErrorKind::NotFound,
            "Config directory not found",
        ));
    }

    // look for a json config file called "service.json" in the config directory
    let config_file = config_dir.join("service.json");

    // read the config file
    let config = std::fs::read_to_string(config_file)?;

    // parse the config file
    let config: ServiceConfig = serde_json::from_str(&config)?;

    Ok(config)
}
