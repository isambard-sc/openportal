// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use anyhow::Error as AnyError;
use anyhow::{Context, Result};
use chrono::serde::ts_seconds;
use chrono::Utc;
use once_cell::sync::Lazy;
use paddington::SecretKey;
use pyo3::exceptions::PyOSError;
use pyo3::prelude::*;
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::path;
use std::sync::RwLock;
use templemeads::job::Status;
use templemeads::server::sign_api_call;
use thiserror::Error;
use url::Url;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BridgeConfig {
    url: Url,
    key: SecretKey,
}

///
/// Load the client configuration from the passed filename.
///
fn local_load_config(config_file: &path::PathBuf) -> Result<(), Error> {
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
            return Err(Error::LockError(format!(
                "Could not get a lock on the config. Error: {:?}",
                e
            )))
        }
    };

    // update the singleton config
    *singleton_config = Some(config);

    Ok(())
}

///
/// Return the current global config object - this will return an error
/// if the config has not been loaded.
///
fn get_config() -> Result<BridgeConfig, Error> {
    let locked_config = match SINGLETON_CONFIG.read() {
        Ok(locked_config) => locked_config,
        Err(e) => {
            return Err(Error::LockError(format!(
                "Could not get a lock on the config. Error: {:?}",
                e
            )))
        }
    };

    let config = match locked_config.as_ref() {
        Some(config) => config,
        None => {
            return Err(Error::InvalidConfig(
                "Config has not been loaded. Please call load_config() first.".to_owned(),
            ))
        }
    };

    Ok(config.clone())
}

// We use the singleton pattern for the BridgeConfig, as we only need to set
// this once, and it will be used by all functions
static SINGLETON_CONFIG: Lazy<RwLock<Option<BridgeConfig>>> = Lazy::new(|| RwLock::new(None));

fn call_get<T>(function: &str) -> Result<T, Error>
where
    T: DeserializeOwned,
{
    tracing::info!("Calling get /{}", function);

    let config = get_config()?;
    let date = Utc::now();

    let url = config.url.join(function).context("Could not join URL")?;

    let auth_token = sign_api_call(&config.key, &date, "get", function, &None)?;

    let result = reqwest::blocking::Client::new()
        .get(url)
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
        Err(Error::CallError(format!(
            "Could not get response for function: {}. Status: {}. Response: {:?}",
            function,
            result.status(),
            result
        )))
    }
}

fn call_post<T>(function: &str, arguments: serde_json::Value) -> Result<T, Error>
where
    T: DeserializeOwned,
{
    tracing::info!("Calling post /{} with arguments: {:?}", function, arguments);

    let config = get_config()?;
    let date = Utc::now();

    let url = config.url.join(function).context("Could not join URL")?;

    let auth_token = sign_api_call(
        &config.key,
        &date,
        "post",
        function,
        &Some(arguments.to_owned()),
    )?;

    let result = reqwest::blocking::Client::new()
        .post(url)
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
        Err(Error::CallError(format!(
            "Could not get response for function: {}. Status: {}. Response: {:?}",
            function,
            result.status(),
            result
        )))
    }
}

///
/// Load the OpenPortal configuration from the passed file
/// and set it as the global configuration.
///
#[pyfunction]
fn load_config(config_file: path::PathBuf) -> PyResult<()> {
    match local_load_config(&config_file) {
        Ok(_) => Ok(()),
        Err(e) => Err(PyErr::new::<PyOSError, _>(format!("{:?}", e))),
    }
}

///
/// Initialize log tracing for the OpenPortal client. This will print
/// logs to stdout.
///
#[pyfunction]
fn initialize_tracing() -> PyResult<()> {
    // Initialize tracing
    let subscriber = tracing_subscriber::FmtSubscriber::new();
    tracing::subscriber::set_global_default(subscriber)
        .expect("Failed to set global default subscriber");
    tracing::info!("Tracing initialized");
    Ok(())
}

///
/// Return type for the health function
///
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

///
/// Return the health of the OpenPortal system.
///
#[pyfunction]
fn health() -> PyResult<Health> {
    tracing::info!("Calling /health");
    match call_get::<Health>("health") {
        Ok(response) => Ok(response),
        Err(e) => Err(PyErr::new::<PyOSError, _>(format!("{:?}", e))),
    }
}

///
/// Return type for the run function. This represents the job being
/// run, and provides functions that let you query the status and
/// get the results
///
#[pyclass]
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Job {
    id: Uuid,
    #[serde(with = "ts_seconds")]
    created: chrono::DateTime<Utc>,
    #[serde(with = "ts_seconds")]
    updated: chrono::DateTime<Utc>,
    version: u64,
    command: String,
    state: Status,
    result: Option<String>,
}

#[pymethods]
impl Job {
    #[getter]
    fn id(&self) -> PyResult<String> {
        Ok(self.id.to_string())
    }

    #[getter]
    fn state(&self) -> PyResult<String> {
        match self.state {
            Status::Pending => Ok("Pending".to_owned()),
            Status::Complete => Ok("Complete".to_owned()),
            Status::Error => Ok("Error".to_owned()),
        }
    }

    #[getter]
    fn command(&self) -> PyResult<String> {
        Ok(self.command.clone())
    }

    #[getter]
    fn created(&self) -> PyResult<String> {
        Ok(self.created.to_rfc3339())
    }

    #[getter]
    fn updated(&self) -> PyResult<String> {
        Ok(self.updated.to_rfc3339())
    }

    #[getter]
    fn result(&self) -> PyResult<Option<String>> {
        Ok(self.result.clone())
    }

    #[getter]
    fn version(&self) -> PyResult<u64> {
        Ok(self.version)
    }

    fn __str__(&self) -> PyResult<String> {
        Ok(format!(
            "Job( command: {}, status: {:?} )",
            self.command, self.state
        ))
    }

    fn __repr__(&self) -> PyResult<String> {
        self.__str__()
    }
}

///
/// Run the passed command on the OpenPortal system.
/// This will return a Job object that can be used to query the
/// status of the job and get the results.
///
#[pyfunction]
fn run(command: String) -> PyResult<Job> {
    match call_post::<Job>("run", serde_json::json!({"command": command})) {
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
pub enum Error {
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
