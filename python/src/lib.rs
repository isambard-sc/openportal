// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use anyhow::Context;
use anyhow::Error as AnyError;
use once_cell::sync::Lazy;
use pyo3::exceptions::PyOSError;
use pyo3::prelude::*;
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::sync::RwLock;
use thiserror::Error;
use tracing_subscriber;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthResponse {
    pub status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HandleResponse {
    pub id: String,
}

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
        .query(&[("openportal-version", "0.01")])
        .header("Accept", "application/json")
        .header("Authorization", config.auth_header())
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
    let config = get_config()?;

    tracing::info!("{}", config.function_path(&func)?);

    let result = reqwest::blocking::Client::new()
        .put(config.function_path(&func)?)
        .header("Accept", "application/json")
        .header("Content-Type", "application/json")
        //.header("Authorization", config.auth_header())
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
    Ok(())
}

#[pyfunction]
fn set_client_config(api_url: String, token: String) -> PyResult<()> {
    let mut config = SINGLETON_CONFIG.write().unwrap();
    config.api_url = url::Url::parse(&api_url).unwrap();
    config.token = token;
    Ok(())
}

#[pyfunction]
fn health() -> PyResult<String> {
    match call_get::<HealthResponse>("health".to_string()) {
        Ok(response) => Ok(response.status),
        Err(e) => Err(PyErr::new::<PyOSError, _>(format!("{:?}", e))),
    }
}

#[pyfunction]
fn run(command: String) -> PyResult<String> {
    match call_put::<HandleResponse>("run".to_owned(), serde_json::json!({"command": command})) {
        Ok(response) => Ok(response.id),
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
