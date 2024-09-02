// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use anyhow::Context;
use anyhow::Error as AnyError;
use chrono::Utc;
use once_cell::sync::Lazy;
use pyo3::exceptions::PyOSError;
use pyo3::prelude::*;
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::sync::RwLock;
use thiserror::Error;
use tracing_subscriber;

#[derive(Debug, Clone)]
pub struct ClientConfig {
    pub api_url: url::Url,
    pub token: String,
}

impl ClientConfig {
    pub fn new() -> Self {
        Self {
            api_url: url::Url::parse("http://localhost:3000/").unwrap(),
            token: "123456789".to_string(),
        }
    }

    pub fn function_path(&self, func: &str) -> Result<url::Url, Error> {
        match self.api_url.join(func) {
            Ok(url) => Ok(url),
            Err(e) => Err(Error::CallError(format!(
                "Could not join function path: {}. Error: {:?}",
                func, e
            ))),
        }
    }

    pub fn auth_header(&self) -> String {
        format!("Token {}", self.token)
    }
}

fn get_config() -> Result<ClientConfig, Error> {
    match SINGLETON_CONFIG.read() {
        Ok(config) => Ok(config.clone()),
        Err(e) => Err(Error::CallError(format!(
            "Could not get a lock on the config. Error: {:?}",
            e
        ))),
    }
}

// We use the singleton pattern for the Client Config, as we only need to set
// this once, and it will be used by all functions
static SINGLETON_CONFIG: Lazy<RwLock<ClientConfig>> =
    Lazy::new(|| RwLock::new(ClientConfig::new()));

fn call_get<T>(func: String) -> Result<T, Error>
where
    T: DeserializeOwned,
{
    let config = get_config()?;

    let result = reqwest::blocking::Client::new()
        .get(config.function_path(&func)?)
        .query(&[("openportal-version", "0.1")])
        .header("Accept", "application/json")
        .header("Authorization", config.auth_header())
        .header(
            "Date",
            Utc::now().format("%a, %d %b %Y %H:%M:%S GMT").to_string(),
        )
        .send()
        .with_context(|| format!("Could not call function: {}", func))?;

    tracing::info!("Response: {:?}", result);

    if result.status().is_success() {
        Ok(result.json::<T>().context("Could not decode from json")?)
    } else {
        Err(Error::CallError(format!(
            "Could not get response for function: {}. Status: {}. Response: {:?}",
            func,
            result.status(),
            result
        )))
    }
}

fn call_put<T>(func: String, body: serde_json::Value) -> Result<T, Error>
where
    T: DeserializeOwned,
{
    tracing::info!("Calling /run with body: {:?}", body);

    let config = get_config()?;

    tracing::info!("{}", config.function_path(&func)?);

    let result = reqwest::blocking::Client::new()
        .post(config.function_path(&func)?)
        .query(&[("openportal-version", "0.1")])
        .header("Accept", "application/json")
        .header("Authorization", config.auth_header())
        .header(
            "Date",
            Utc::now().format("%a, %d %b %Y %H:%M:%S GMT").to_string(),
        )
        .json(&body)
        .send()
        .with_context(|| format!("Could not call function: {}", func))?;

    tracing::info!("Response: {:?}", result);

    if result.status().is_success() {
        Ok(result.json::<T>().context("Could not decode from json")?)
    } else {
        Err(Error::CallError(format!(
            "Could not get response for function: {}. Status: {}. Response: {:?}",
            func,
            result.status(),
            result
        )))
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

#[pyfunction]
fn set_client_config(api_url: String, token: String) -> PyResult<()> {
    tracing::info!("Updating the client config: {}", api_url);
    let mut config = SINGLETON_CONFIG.write().unwrap();
    config.api_url = url::Url::parse(&api_url).unwrap();
    config.token = token;
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
    match call_put::<Job>("run".to_owned(), serde_json::json!({"command": command})) {
        Ok(response) => Ok(response),
        Err(e) => Err(PyErr::new::<PyOSError, _>(format!("{:?}", e))),
    }
}

#[pymodule]
fn openportal(m: &Bound<'_, PyModule>) -> PyResult<()> {
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
    CallError(String),

    #[error("Unknown error")]
    Unknown,
}
