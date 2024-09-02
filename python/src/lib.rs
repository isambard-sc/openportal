// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use anyhow::Context;
use anyhow::Error as AnyError;
use chrono::Utc;
use once_cell::sync::Lazy;
use paddington::{CryptoError, Key, SecretKey, Signature};
use pyo3::exceptions::PyOSError;
use pyo3::prelude::*;
use secrecy::ExposeSecret;
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::error;
use std::fmt::Result;
use std::path;
use std::sync::RwLock;
use templemeads::sign_api_call;
use thiserror::Error;
use tracing_subscriber;
use url::Url;

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct BridgeConfig {
    api_url: Option<Url>,
    key: Option<SecretKey>,
}

impl BridgeConfig {
    pub fn new() -> Self {
        Self {
            api_url: None,
            key: None,
        }
    }
}

fn get_config() -> Result<BridgeConfig, OPError> {
    match SINGLETON_CONFIG.read() {
        Ok(config) => Ok(config.clone()),
        Err(e) => Err(OPError::InvalidConfig(format!(
            "Could not get a lock on the config. Error: {:?}",
            e
        ))),
    }
}

// We use the singleton pattern for the BridgeConfig, as we only need to set
// this once, and it will be used by all functions
static SINGLETON_CONFIG: Lazy<RwLock<BridgeConfig>> =
    Lazy::new(|| RwLock::new(BridgeConfig::default()));

fn call_get<T>(function: String) -> Result<T, OPError>
where
    T: DeserializeOwned,
{
    tracing::info!("Calling get /{}", function);

    let config = get_config()?;
    let date = Utc::now();

    let protocol = "get".to_string();

    let api_url = format!(
        "{}/{}",
        config
            .api_url
            .ok_or(OPError::InvalidConfig(format!("Missing config URL")))?,
        function
    );

    let auth_token = sign_api_call(
        &config
            .key
            .ok_or(OPError::InvalidConfig(format!("Missing config key")))?,
        date,
        protocol,
        function,
        None,
    )?;

    let result = reqwest::blocking::Client::new()
        .get(api_url)
        .query(&[("openportal-version", "0.1")])
        .header("Accept", "application/json")
        .header("Authorization", auth_token)
        .header("Date", date.format("%a, %d %b %Y %H:%M:%S GMT").to_string())
        .send()
        .with_context(|| format!("Could not call function: {}", function))?;

    tracing::info!("Response: {:?}", result);

    if result.status().is_success() {
        Ok(result.json::<T>().context("Could not decode from json")?)
    } else {
        Err(OPError::CallError(format!(
            "Could not get response for function: {}. Status: {}. Response: {:?}",
            function,
            result.status(),
            result
        )))
    }
}

fn call_post<T>(function: String, arguments: serde_json::Value) -> Result<T, OPError>
where
    T: DeserializeOwned,
{
    tracing::info!("Calling post /{} with arguments: {:?}", function, arguments);

    tracing::info!("Calling get /{}", function);

    let config = get_config()?;
    let date = Utc::now();

    let protocol = "put".to_string();

    let api_url = format!(
        "{}/{}",
        config
            .api_url
            .ok_or(OPError::InvalidConfig(format!("Missing config URL")))?,
        function
    );

    let auth_token = sign_api_call(
        &config
            .key
            .ok_or(OPError::InvalidConfig(format!("Missing config key")))?,
        date,
        protocol,
        function,
        Some(arguments),
    )?;

    let result = reqwest::blocking::Client::new()
        .post(api_url)
        .query(&[("openportal-version", "0.1")])
        .header("Accept", "application/json")
        .header("Authorization", auth_token)
        .header("Date", date.format("%a, %d %b %Y %H:%M:%S GMT").to_string())
        .json(&arguments)
        .send()
        .with_context(|| format!("Could not call function: {}", function))?;

    tracing::info!("Response: {:?}", result);

    if result.status().is_success() {
        Ok(result.json::<T>().context("Could not decode from json")?)
    } else {
        Err(OPError::CallError(format!(
            "Could not get response for function: {}. Status: {}. Response: {:?}",
            function,
            result.status(),
            result
        )))
    }
}

///
/// Load the client configuration from the passed filename.
///
fn local_load_config(config_file: &path::PathBuf) -> Result<(), OPError> {
    // see if this config_file exists - return an error if it doesn't
    let config_file = path::absolute(config_file)?;

    // read the config file
    let config = std::fs::read_to_string(&config_file)
        .with_context(|| format!("Could not read config file: {:?}", config_file))?;

    // parse the config file
    let config: BridgeConfig = toml::from_str(&config)
        .with_context(|| format!("Could not parse config file fron toml: {:?}", config_file))?;

    let mut singleton_config = match SINGLETON_CONFIG.write() {
        Ok(guard) => guard,
        Err(e) => {
            return Err(OPError::LockError(format!(
                "Could not get a lock on the config. Error: {:?}",
                e
            )))
        }
    };

    // update the singleton config
    *singleton_config = config;

    Ok(())
}

#[pyfunction]
fn load_config(config_file: path::PathBuf) -> PyResult<()> {
    match local_load_config(&config_file) {
        Ok(_) => Ok(()),
        Err(e) => Err(PyErr::new::<PyOSError, _>(format!("{:?}", e))),
    }
}

#[pyfunction]
fn initialize_tracing() -> PyResult<()> {
    // Initialize tracing
    let subscriber = tracing_subscriber::FmtSubscriber::new();
    tracing::subscriber::set_global_default(subscriber)
        .expect("Failed to set global default subscriber");
    tracing::info!("Tracing initialized");
    Ok(())
}

#[pyclass]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Health {
    pub status: String,
}

#[pymethods]
impl Health {
    #[getter]
    fn status(&self) -> PyResult<String> {
        Ok(self.status.clone())
    }

    fn __str__(&self) -> PyResult<String> {
        Ok(format!("Health( status: {} )", self.status))
    }

    fn __repr__(&self) -> PyResult<String> {
        self.__str__()
    }

    fn is_healthy(&self) -> PyResult<bool> {
        Ok(self.status == "ok")
    }
}

#[pyfunction]
fn health() -> PyResult<Health> {
    tracing::info!("Calling /health");
    match call_get::<Health>("health".to_string()) {
        Ok(response) => Ok(response),
        Err(e) => Err(PyErr::new::<PyOSError, _>(format!("{:?}", e))),
    }
}

#[pyclass]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Job {
    id: String,
    status: String,
}

#[pymethods]
impl Job {
    #[getter]
    fn id(&self) -> PyResult<String> {
        Ok(self.id.clone())
    }

    #[getter]
    fn status(&self) -> PyResult<String> {
        Ok(self.status.clone())
    }

    fn __str__(&self) -> PyResult<String> {
        Ok(format!("Job( id: {}, status: {} )", self.id, self.status))
    }

    fn __repr__(&self) -> PyResult<String> {
        self.__str__()
    }
}

#[pyfunction]
fn run(command: String) -> PyResult<Job> {
    match call_post::<Job>("run".to_owned(), serde_json::json!({"command": command})) {
        Ok(response) => Ok(response),
        Err(e) => Err(PyErr::new::<PyOSError, _>(format!("{:?}", e))),
    }
}

#[pymodule]
fn openportal(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(load_config, m)?)?;
    m.add_function(wrap_pyfunction!(initialize_tracing, m)?)?;
    m.add_function(wrap_pyfunction!(health, m)?)?;
    m.add_function(wrap_pyfunction!(run, m)?)?;
    Ok(())
}

/// Errors

#[derive(Error, Debug)]
pub enum OPError {
    #[error("{0}")]
    AnyError(#[from] AnyError),

    #[error("{0}")]
    IOError(#[from] std::io::Error),

    #[error("{0}")]
    CallError(String),

    #[error("File does not exist: {0}")]
    NotExists(path::PathBuf),

    #[error("{0}")]
    InvalidConfig(String),

    #[error("{0}")]
    LockError(String),

    #[error("Unknown error")]
    Unknown,
}
