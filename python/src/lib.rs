// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

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
use templemeads::Error;
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
        Err(Error::Call(format!(
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
        Err(Error::Call(format!(
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
/// Return whether or not a valid configuration has been loaded
///
#[pyfunction]
fn is_config_loaded() -> PyResult<bool> {
    match SINGLETON_CONFIG.read() {
        Ok(guard) => Ok(guard.is_some()),
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
    changed: chrono::DateTime<Utc>,
    #[serde(with = "ts_seconds")]
    expires: chrono::DateTime<Utc>,
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

    fn _is_expired(&self) -> bool {
        match self.state {
            Status::Complete => false,
            Status::Error => false,
            _ => Utc::now() > self.expires,
        }
    }

    #[getter]
    fn state(&self) -> PyResult<String> {
        if self._is_expired() {
            match self.state {
                Status::Complete => Ok("Complete".to_owned()),
                _ => Ok("Error".to_owned()),
            }
        } else {
            match self.state {
                Status::Created => Ok("Created".to_owned()),
                Status::Pending => Ok("Pending".to_owned()),
                Status::Running => Ok("Running".to_owned()),
                Status::Complete => Ok("Complete".to_owned()),
                Status::Error => Ok("Error".to_owned()),
            }
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
    fn changed(&self) -> PyResult<String> {
        Ok(self.changed.to_rfc3339())
    }

    #[getter]
    fn expires(&self) -> PyResult<String> {
        Ok(self.expires.to_rfc3339())
    }

    #[getter]
    fn result(&self) -> PyResult<Option<String>> {
        if self.state == Status::Complete {
            Ok(self.result.clone())
        } else {
            Ok(None)
        }
    }

    #[getter]
    fn is_finished(&self) -> PyResult<bool> {
        Ok(self.state == Status::Complete || self.state == Status::Error || self._is_expired())
    }

    #[getter]
    fn is_expired(&self) -> PyResult<bool> {
        Ok(self._is_expired())
    }

    #[getter]
    fn is_error(&self) -> PyResult<bool> {
        match self._is_expired() {
            true => Ok(self.state != Status::Complete),
            false => Ok(self.state == Status::Error),
        }
    }

    #[getter]
    fn error_message(&self) -> PyResult<Option<String>> {
        match self._is_expired() {
            true => {
                if self.state == Status::Error {
                    Ok(self.result.clone())
                } else if self.state == Status::Complete {
                    Ok(None)
                } else {
                    Ok(Some("Job has expired".to_owned()))
                }
            }
            false => {
                if self.state == Status::Error {
                    Ok(self.result.clone())
                } else {
                    Ok(None)
                }
            }
        }
    }

    #[getter]
    fn progress_message(&self) -> PyResult<String> {
        match self._is_expired() {
            true => match self.state {
                Status::Complete => Ok("Complete".to_owned()),
                Status::Error => Ok("Error".to_owned()),
                _ => Ok("Error (expired)".to_owned()),
            },
            false => match self.state {
                Status::Running => {
                    if let Some(result) = &self.result {
                        Ok(result.clone())
                    } else {
                        Ok("Running".to_owned())
                    }
                }
                Status::Created => Ok("Created".to_owned()),
                Status::Pending => Ok("Pending".to_owned()),
                Status::Complete => Ok("Complete".to_owned()),
                Status::Error => Ok("Error".to_owned()),
            },
        }
    }

    #[getter]
    fn version(&self) -> PyResult<u64> {
        Ok(self.version)
    }

    fn update(&mut self) -> PyResult<()> {
        // don't update if the job is already finished
        if self.is_finished()? {
            return Ok(());
        }

        match status(self.clone()) {
            Ok(updated) => {
                *self = updated;
                Ok(())
            }
            Err(e) => Err(PyErr::new::<PyOSError, _>(format!("{:?}", e))),
        }
    }

    fn __str__(&self) -> PyResult<String> {
        match self._is_expired() {
            true => match self.state {
                Status::Complete => Ok(format!(
                    "Job( command: {}, status: completed, result: {} )",
                    self.command,
                    self.result.clone().unwrap_or("None".to_owned())
                )),
                Status::Error => Ok(format!(
                    "Job( command: {}, status: error, message: {} )",
                    self.command,
                    self.result.clone().unwrap_or("None".to_owned())
                )),
                _ => Ok(format!(
                    "Job( command: {}, status: error, message: Job has expired )",
                    self.command
                )),
            },
            false => match self.state {
                Status::Complete => Ok(format!(
                    "Job( command: {}, status: completed, result: {} )",
                    self.command,
                    self.result.clone().unwrap_or("None".to_owned())
                )),
                Status::Error => Ok(format!(
                    "Job( command: {}, status: error, message: {} )",
                    self.command,
                    self.result.clone().unwrap_or("None".to_owned())
                )),
                _ => Ok(format!(
                    "Job( command: {}, status: {:?}, message: {} )",
                    self.command,
                    self.state,
                    self.result.clone().unwrap_or("None".to_owned())
                )),
            },
        }
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

///
/// Get the status of the passed job on the OpenPortal System
/// This will return the job updated to the latest version.
///
#[pyfunction]
fn status(job: Job) -> PyResult<Job> {
    match call_post::<Job>("status", serde_json::json!({"job": job.id})) {
        Ok(response) => Ok(response),
        Err(e) => Err(PyErr::new::<PyOSError, _>(format!("{:?}", e))),
    }
}

///
/// Return the Job with the specified ID. Raises an error if the
/// job does not exist.
///
#[pyfunction]
fn get(job_id: &str) -> PyResult<Job> {
    match call_post::<Job>("status", serde_json::json!({"job": job_id.to_string()})) {
        Ok(response) => Ok(response),
        Err(e) => Err(PyErr::new::<PyOSError, _>(format!("{:?}", e))),
    }
}

#[pymodule]
fn openportal(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(load_config, m)?)?;
    m.add_function(wrap_pyfunction!(is_config_loaded, m)?)?;
    m.add_function(wrap_pyfunction!(initialize_tracing, m)?)?;
    m.add_function(wrap_pyfunction!(health, m)?)?;
    m.add_function(wrap_pyfunction!(run, m)?)?;
    m.add_function(wrap_pyfunction!(status, m)?)?;
    m.add_function(wrap_pyfunction!(get, m)?)?;
    Ok(())
}
