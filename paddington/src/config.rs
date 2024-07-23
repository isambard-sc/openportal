// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use crate::crypto::{Key, SecretKey};
use serde::{Deserialize, Serialize};
use serde_json;
use std::io::Error as IOError;
use std::path;

///
/// The full service configuration.
/// This includes the service name, the secret key, the server to bind to and the port to bind to.
/// The secret key is generated when the service is created and is used to encrypt and decrypt messages.
/// The server is the address that the service will bind to and the port is the port that the service will bind to.
/// The service name is the name of the service.
///
/// # Example
///
/// ```
/// use paddington::config::ServiceConfig;
/// use paddington::crypto::Key;
///
/// let config = ServiceConfig {
///    name: "openportal".to_string(),
///    key: Key::generate(),
///    server: "localhost".to_string(),
///    port: 8080,
/// };
/// ```
///
#[derive(Serialize, Deserialize, Debug)]
pub struct ServiceConfig {
    pub name: String,
    pub key: SecretKey,
    pub server: String,
    pub port: u16,
}

///
/// Create a new service configuration in the passed directory.
/// This will return an error if the config directory already exists.
/// The service name, server and port can be passed as arguments.
/// If they are not passed, default values will be used.
/// The service name will default to "openportal", the server will default to "localhost"
/// and the port will default to 8080.
///
/// # Arguments
///
/// * `config_dir` - The directory to create the service configuration in.
/// * `service_name` - The name of the service.
/// * `server` - The server to bind to.
/// * `port` - The port to bind to.
///
/// # Returns
///
/// The full service configuration.
///
/// # Errors
///
/// This function will return an error if the config directory already exists.
///
/// # Example
///
/// ```
/// use paddington::config;
///
/// let config = config::create("/path/to/config", "service_name",
///                             "https://service_url", 8000)?;
///
/// println!("Service name: {}", config.name);
/// ```
///
pub fn create(
    config_dir: &path::PathBuf,
    service_name: &Option<String>,
    server: &Option<String>,
    port: &Option<u16>,
) -> Result<ServiceConfig, IOError> {
    // see if this config_dir exists - return an error if it does
    let config_dir = path::absolute(config_dir)?;

    if config_dir.try_exists()? {
        return Err(IOError::new(
            std::io::ErrorKind::AlreadyExists,
            "Config directory already exists",
        ));
    }

    // create the config directory
    println!("Creating config directory: {:?}", config_dir);
    std::fs::create_dir_all(&config_dir)?;
    println!("Created config directory: {:?}", config_dir);

    let service_name = service_name.clone().unwrap_or("openportal".to_string());
    let server = server.clone().unwrap_or("localhost".to_string());
    let port = port.unwrap_or(8080);

    let config = ServiceConfig {
        name: service_name.clone(),
        key: Key::generate(),
        server: server.clone(),
        port,
    };

    // write the config to a json file
    let config_file = config_dir.join("service.json");
    println!("Writing config file: {:?}", config_file);
    let config = serde_json::to_string(&config)?;
    std::fs::write(config_file, config)?;

    let encrypted_config = Key::generate().encrypt(config);

    println!("Encrypted config = {:?}", encrypted_config);

    // read the config and return it
    load(&config_dir)
}

///
/// Load the full service configuration from the passed directory.
/// This will return an error if the config directory does not exist
/// or if the data within cannot be read.
///
/// # Arguments
///
/// * `config_dir` - The directory containing the service configuration.
///
/// # Returns
///
/// The full service configuration.
///
/// # Errors
///
/// This function will return an error if the config directory does not exist
/// or if the data within cannot be read.
///
/// # Example
///
/// ```
/// use paddington::config;
///
/// let config = config::load("/path/to/config")?;
///
/// println!("Service name: {}", config.name);
/// ```
///
pub fn load(config_dir: &path::PathBuf) -> Result<ServiceConfig, IOError> {
    // see if this config_dir exists - return an error if it doesn't
    let config_dir = path::absolute(config_dir)?;

    if !config_dir.try_exists()? {
        return Err(IOError::new(
            std::io::ErrorKind::NotFound,
            "Config directory not found",
        ));
    }

    // look for a json config file called "service.json" in the config directory
    let config_file = config_dir.join("service.json");

    // read the config file
    println!("Reading config file: {:?}", config_file);
    let config = std::fs::read_to_string(config_file)?;

    // parse the config file
    let config: ServiceConfig = serde_json::from_str(&config)?;

    Ok(config)
}
