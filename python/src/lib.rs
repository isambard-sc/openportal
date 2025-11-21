// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use anyhow::{Context, Result};
use chrono::Utc;
use once_cell::sync::Lazy;
use paddington::SecretKey;
use pyo3::basic::CompareOp;
use pyo3::exceptions::PyOSError;
use pyo3::prelude::*;
use pyo3::types::{PyDate, PyDateTime, PyList, PyString, PyTzInfo};
use pyo3::{IntoPyObject, PyResult, Python};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::collections::HashMap;
use std::path;
use std::sync::RwLock;
use templemeads::destination;
use templemeads::diagnostics as mod_diagnostics;
use templemeads::grammar;
use templemeads::health as mod_health;
use templemeads::job;
use templemeads::server::sign_api_call;
use templemeads::usagereport;
use templemeads::Error;
use url::Url;

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
            return Err(Error::Locked(format!(
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
            return Err(Error::Locked(format!(
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
    tracing::debug!("Calling get /{}", function);

    let config = get_config()?;

    // Retry logic with exponential backoff for rate limiting
    const MAX_RETRIES: u32 = 5;
    const INITIAL_BACKOFF_MS: u64 = 100;

    for attempt in 0..=MAX_RETRIES {
        let date = Utc::now();
        let url = config.url.join(function).context("Could not join URL")?;

        // Generate a unique nonce for replay attack prevention
        let nonce = uuid::Uuid::new_v4().to_string();
        let auth_token = sign_api_call(&config.key, &date, "get", function, &None, Some(&nonce))?;

        let result = reqwest::blocking::Client::new()
            .get(url)
            .query(&[("openportal-version", "0.1")])
            .header("Accept", "application/json")
            .header("Authorization", auth_token)
            .header("Date", date.format("%a, %d %b %Y %H:%M:%S GMT").to_string())
            .header("X-Nonce", nonce)
            .send()
            .with_context(|| format!("Could not call function: {}", function))?;

        tracing::debug!("Response: {:?}", result);

        if result.status().is_success() {
            return Ok(result.json::<T>().context("Could not decode from json")?);
        } else if result.status() == reqwest::StatusCode::TOO_MANY_REQUESTS && attempt < MAX_RETRIES
        {
            // Rate limited - backoff and retry
            let backoff_ms = INITIAL_BACKOFF_MS * 2_u64.pow(attempt);
            tracing::warn!(
                "Rate limited on attempt {} for function: {}. Backing off for {}ms",
                attempt + 1,
                function,
                backoff_ms
            );
            std::thread::sleep(std::time::Duration::from_millis(backoff_ms));
        } else {
            return Err(Error::Call(format!(
                "Could not get response for function: {}. Status: {}. Response: {:?}",
                function,
                result.status(),
                result
            )));
        }
    }

    // If we exhausted all retries
    Err(Error::Call(format!(
        "Exceeded maximum retries ({}) for function: {} due to rate limiting",
        MAX_RETRIES, function
    )))
}

fn call_post<T>(function: &str, arguments: serde_json::Value) -> Result<T, Error>
where
    T: DeserializeOwned,
{
    tracing::debug!("Calling post /{} with arguments: {:?}", function, arguments);

    let config = get_config()?;

    // Retry logic with exponential backoff for rate limiting
    const MAX_RETRIES: u32 = 5;
    const INITIAL_BACKOFF_MS: u64 = 100;

    for attempt in 0..=MAX_RETRIES {
        let date = Utc::now();
        let url = config.url.join(function).context("Could not join URL")?;

        // Generate a unique nonce for replay attack prevention
        let nonce = uuid::Uuid::new_v4().to_string();
        let auth_token = sign_api_call(
            &config.key,
            &date,
            "post",
            function,
            &Some(arguments.to_owned()),
            Some(&nonce),
        )?;

        let result = reqwest::blocking::Client::new()
            .post(url)
            .query(&[("openportal-version", "0.1")])
            .header("Accept", "application/json")
            .header("Authorization", auth_token)
            .header("Date", date.format("%a, %d %b %Y %H:%M:%S GMT").to_string())
            .header("X-Nonce", nonce)
            .json(&arguments)
            .send()
            .with_context(|| format!("Could not call function: {}", function))?;

        tracing::debug!("Response: {:?}", result);

        if result.status().is_success() {
            return Ok(result.json::<T>().context("Could not decode from json")?);
        } else if result.status() == reqwest::StatusCode::TOO_MANY_REQUESTS && attempt < MAX_RETRIES
        {
            // Rate limited - backoff and retry
            let backoff_ms = INITIAL_BACKOFF_MS * 2_u64.pow(attempt);
            tracing::warn!(
                "Rate limited on attempt {} for function: {}. Backing off for {}ms",
                attempt + 1,
                function,
                backoff_ms
            );
            std::thread::sleep(std::time::Duration::from_millis(backoff_ms));
        } else {
            return Err(Error::Call(format!(
                "Could not get response for function: {}. Status: {}. Response: {:?}",
                function,
                result.status(),
                result
            )));
        }
    }

    // If we exhausted all retries
    Err(Error::Call(format!(
        "Exceeded maximum retries ({}) for function: {} due to rate limiting",
        MAX_RETRIES, function
    )))
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
    templemeads::config::initialise_tracing();
    Ok(())
}

///
/// The FailedJobEntry object for diagnostics reports
///
#[pyclass(module = "openportal")]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FailedJobEntry(mod_diagnostics::FailedJobEntry);

#[pymethods]
impl FailedJobEntry {
    fn __str__(&self) -> PyResult<String> {
        Ok(self.0.to_string())
    }

    fn __repr__(&self) -> PyResult<String> {
        self.__str__()
    }

    #[getter]
    fn destination(&self) -> PyResult<String> {
        Ok(self.0.destination.clone())
    }

    #[getter]
    fn instruction(&self) -> PyResult<String> {
        Ok(self.0.instruction.clone())
    }

    #[getter]
    fn error_message(&self) -> PyResult<String> {
        Ok(self.0.error_message.clone())
    }

    #[getter]
    fn count(&self) -> PyResult<usize> {
        Ok(self.0.count)
    }

    #[getter]
    fn first_seen<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyDateTime>> {
        PyDateTime::from_timestamp(
            py,
            self.0.first_seen.timestamp() as f64,
            PyTzInfo::utc(py).ok().as_deref(),
        )
    }

    #[getter]
    fn last_seen<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyDateTime>> {
        PyDateTime::from_timestamp(
            py,
            self.0.last_seen.timestamp() as f64,
            PyTzInfo::utc(py).ok().as_deref(),
        )
    }

    fn __copy__(&self) -> PyResult<FailedJobEntry> {
        Ok(self.clone())
    }

    fn __deepcopy__(&self, _memo: Py<PyAny>) -> PyResult<FailedJobEntry> {
        Ok(self.clone())
    }
}

impl From<mod_diagnostics::FailedJobEntry> for FailedJobEntry {
    fn from(diagnostics_report: mod_diagnostics::FailedJobEntry) -> Self {
        FailedJobEntry(diagnostics_report)
    }
}

///
/// The SlowJobEntry object for diagnostics reports
///
#[pyclass(module = "openportal")]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SlowJobEntry(mod_diagnostics::SlowJobEntry);

#[pymethods]
impl SlowJobEntry {
    fn __str__(&self) -> PyResult<String> {
        Ok(self.0.to_string())
    }

    fn __repr__(&self) -> PyResult<String> {
        self.__str__()
    }

    #[getter]
    fn destination(&self) -> PyResult<String> {
        Ok(self.0.destination.clone())
    }

    #[getter]
    fn instruction(&self) -> PyResult<String> {
        Ok(self.0.instruction.clone())
    }

    #[getter]
    fn duration_ms(&self) -> PyResult<f64> {
        Ok(self.0.duration_ms)
    }

    #[getter]
    fn completed_at<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyDateTime>> {
        PyDateTime::from_timestamp(
            py,
            self.0.completed_at.timestamp() as f64,
            PyTzInfo::utc(py).ok().as_deref(),
        )
    }

    fn __copy__(&self) -> PyResult<SlowJobEntry> {
        Ok(self.clone())
    }

    fn __deepcopy__(&self, _memo: Py<PyAny>) -> PyResult<SlowJobEntry> {
        Ok(self.clone())
    }
}

impl From<mod_diagnostics::SlowJobEntry> for SlowJobEntry {
    fn from(diagnostics_report: mod_diagnostics::SlowJobEntry) -> Self {
        SlowJobEntry(diagnostics_report)
    }
}

///
/// The ExpiredJobEntry object for diagnostics reports
///
#[pyclass(module = "openportal")]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExpiredJobEntry(mod_diagnostics::ExpiredJobEntry);

#[pymethods]
impl ExpiredJobEntry {
    fn __str__(&self) -> PyResult<String> {
        Ok(self.0.to_string())
    }

    fn __repr__(&self) -> PyResult<String> {
        self.__str__()
    }

    #[getter]
    fn destination(&self) -> PyResult<String> {
        Ok(self.0.destination.clone())
    }

    #[getter]
    fn instruction(&self) -> PyResult<String> {
        Ok(self.0.instruction.clone())
    }

    #[getter]
    fn created_at<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyDateTime>> {
        PyDateTime::from_timestamp(
            py,
            self.0.created_at.timestamp() as f64,
            PyTzInfo::utc(py).ok().as_deref(),
        )
    }

    #[getter]
    fn expired_at<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyDateTime>> {
        PyDateTime::from_timestamp(
            py,
            self.0.expired_at.timestamp() as f64,
            PyTzInfo::utc(py).ok().as_deref(),
        )
    }

    #[getter]
    fn count(&self) -> PyResult<usize> {
        Ok(self.0.count)
    }

    fn __copy__(&self) -> PyResult<ExpiredJobEntry> {
        Ok(self.clone())
    }

    fn __deepcopy__(&self, _memo: Py<PyAny>) -> PyResult<ExpiredJobEntry> {
        Ok(self.clone())
    }
}

impl From<mod_diagnostics::ExpiredJobEntry> for ExpiredJobEntry {
    fn from(diagnostics_report: mod_diagnostics::ExpiredJobEntry) -> Self {
        ExpiredJobEntry(diagnostics_report)
    }
}

///
/// The RunningJobEntry object for diagnostics reports
///
#[pyclass(module = "openportal")]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunningJobEntry(mod_diagnostics::RunningJobEntry);

#[pymethods]
impl RunningJobEntry {
    fn __str__(&self) -> PyResult<String> {
        Ok(self.0.to_string())
    }

    fn __repr__(&self) -> PyResult<String> {
        self.__str__()
    }

    #[getter]
    fn destination(&self) -> PyResult<String> {
        Ok(self.0.destination.clone())
    }

    #[getter]
    fn instruction(&self) -> PyResult<String> {
        Ok(self.0.instruction.clone())
    }

    #[getter]
    fn started_at<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyDateTime>> {
        PyDateTime::from_timestamp(
            py,
            self.0.started_at.timestamp() as f64,
            PyTzInfo::utc(py).ok().as_deref(),
        )
    }

    #[getter]
    fn count(&self) -> PyResult<usize> {
        Ok(self.0.count)
    }

    #[getter]
    fn running_for_seconds(&self) -> PyResult<i64> {
        Ok(self.0.running_for_seconds)
    }

    fn __copy__(&self) -> PyResult<RunningJobEntry> {
        Ok(self.clone())
    }

    fn __deepcopy__(&self, _memo: Py<PyAny>) -> PyResult<RunningJobEntry> {
        Ok(self.clone())
    }
}

impl From<mod_diagnostics::RunningJobEntry> for RunningJobEntry {
    fn from(diagnostics_report: mod_diagnostics::RunningJobEntry) -> Self {
        RunningJobEntry(diagnostics_report)
    }
}

///
/// The DiagnosticsReport object returned from diagnostics requests
///
#[pyclass(module = "openportal")]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiagnosticsReport(mod_diagnostics::DiagnosticsReport);

#[pymethods]
impl DiagnosticsReport {
    #[getter]
    fn agent_name(&self) -> PyResult<String> {
        Ok(self.0.agent_name.clone())
    }

    #[getter]
    fn generated_at<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyDateTime>> {
        PyDateTime::from_timestamp(
            py,
            self.0.generated_at.timestamp() as f64,
            PyTzInfo::utc(py).ok().as_deref(),
        )
    }

    #[getter]
    fn failed_jobs(&self) -> PyResult<Vec<FailedJobEntry>> {
        Ok(self.0.failed_jobs.iter().cloned().map(Into::into).collect())
    }

    #[getter]
    fn slowest_jobs(&self) -> PyResult<Vec<SlowJobEntry>> {
        Ok(self
            .0
            .slowest_jobs
            .iter()
            .cloned()
            .map(Into::into)
            .collect())
    }

    #[getter]
    fn expired_jobs(&self) -> PyResult<Vec<ExpiredJobEntry>> {
        Ok(self
            .0
            .expired_jobs
            .iter()
            .cloned()
            .map(Into::into)
            .collect())
    }

    #[getter]
    fn running_jobs(&self) -> PyResult<Vec<RunningJobEntry>> {
        Ok(self
            .0
            .running_jobs
            .iter()
            .cloned()
            .map(Into::into)
            .collect())
    }

    #[getter]
    fn warnings(&self) -> PyResult<Vec<String>> {
        Ok(self.0.warnings.clone())
    }

    fn __str__(&self) -> PyResult<String> {
        Ok(self.0.to_pretty_string())
    }

    fn __repr__(&self) -> PyResult<String> {
        self.__str__()
    }

    fn __copy__(&self) -> PyResult<DiagnosticsReport> {
        Ok(self.clone())
    }

    fn __deepcopy__(&self, _memo: Py<PyAny>) -> PyResult<DiagnosticsReport> {
        Ok(self.clone())
    }
}

impl From<mod_diagnostics::DiagnosticsReport> for DiagnosticsReport {
    fn from(diagnostics_report: mod_diagnostics::DiagnosticsReport) -> Self {
        DiagnosticsReport(diagnostics_report)
    }
}

///
/// Return type for the diagnostics function
///
#[pyclass(module = "openportal")]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Diagnostics {
    pub status: String,
    #[serde(default)]
    #[serde(rename = "report")]
    pub diagnostics: Option<DiagnosticsReport>,
}

#[pymethods]
impl Diagnostics {
    #[getter]
    fn status(&self) -> PyResult<String> {
        Ok(self.status.clone())
    }

    #[getter]
    fn detail(&self) -> PyResult<Option<DiagnosticsReport>> {
        Ok(self.diagnostics.clone())
    }

    fn __str__(&self) -> PyResult<String> {
        let mut s = format!("Diagnostics( status: {}", self.status);
        if let Some(ref diagnostics) = self.diagnostics {
            s.push_str(&format!(
                ", detail:\n{}\n",
                diagnostics.0.to_pretty_string()
            ));
        }
        s.push_str(" )");
        Ok(s)
    }

    fn __repr__(&self) -> PyResult<String> {
        self.__str__()
    }

    fn __copy__(&self) -> PyResult<Diagnostics> {
        Ok(self.clone())
    }

    fn __deepcopy__(&self, _memo: Py<PyAny>) -> PyResult<Diagnostics> {
        Ok(self.clone())
    }

    fn is_healthy(&self) -> PyResult<bool> {
        Ok(self.status == "ok")
    }
}

///
/// Fetch the diagnostics report from an agent in the OpenPortal system.
///
/// Parameters:
/// - destination: Dot-separated path to the agent (e.g., "brics.aip2.clusters")
///                Empty string means get the diagnostics from the bridge itself.
///
#[pyfunction]
fn diagnostics(destination: &str) -> PyResult<Diagnostics> {
    tracing::debug!("Calling /diagnostics with destination={}", destination);

    let params = serde_json::json!({
        "destination": destination,
    });

    match call_post::<Diagnostics>("diagnostics", params) {
        Ok(response) => Ok(response),
        Err(e) => Err(PyErr::new::<PyOSError, _>(format!("{:?}", e))),
    }
}

///
/// The HealthInfo object for each of the agent health checks
///
#[pyclass(module = "openportal")]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthInfo(mod_health::HealthInfo);

#[pymethods]
impl HealthInfo {
    #[getter]
    fn name(&self) -> PyResult<String> {
        Ok(self.0.name.clone())
    }

    #[getter]
    fn agent_type(&self) -> PyResult<String> {
        Ok(self.0.agent_type.to_string())
    }

    #[getter]
    fn connected(&self) -> PyResult<bool> {
        Ok(self.0.connected)
    }

    #[getter]
    fn active_jobs(&self) -> PyResult<u64> {
        Ok(self.0.active_jobs as u64)
    }

    #[getter]
    fn pending_jobs(&self) -> PyResult<u64> {
        Ok(self.0.pending_jobs as u64)
    }

    #[getter]
    fn running_jobs(&self) -> PyResult<u64> {
        Ok(self.0.running_jobs as u64)
    }

    #[getter]
    fn completed_jobs(&self) -> PyResult<u64> {
        Ok(self.0.completed_jobs as u64)
    }

    #[getter]
    fn successful_jobs(&self) -> PyResult<u64> {
        Ok(self.0.successful_jobs as u64)
    }

    #[getter]
    fn expired_jobs(&self) -> PyResult<u64> {
        Ok(self.0.expired_jobs as u64)
    }

    #[getter]
    fn errored_jobs(&self) -> PyResult<u64> {
        Ok(self.0.errored_jobs as u64)
    }

    #[getter]
    fn duplicate_jobs(&self) -> PyResult<u64> {
        Ok(self.0.duplicate_jobs as u64)
    }

    #[getter]
    fn worker_count(&self) -> PyResult<u64> {
        Ok(self.0.worker_count as u64)
    }

    #[getter]
    fn memory_bytes(&self) -> PyResult<u64> {
        Ok(self.0.memory_bytes)
    }

    #[getter]
    fn cpu_percent(&self) -> PyResult<f32> {
        Ok(self.0.cpu_percent)
    }

    #[getter]
    fn system_memory_total(&self) -> PyResult<u64> {
        Ok(self.0.system_memory_total)
    }

    #[getter]
    fn system_cpus(&self) -> PyResult<u32> {
        Ok(self.0.system_cpus as u32)
    }

    #[getter]
    fn job_time_min_ms(&self) -> PyResult<f64> {
        Ok(self.0.job_time_min_ms)
    }

    #[getter]
    fn job_time_max_ms(&self) -> PyResult<f64> {
        Ok(self.0.job_time_max_ms)
    }

    #[getter]
    fn job_time_mean_ms(&self) -> PyResult<f64> {
        Ok(self.0.job_time_mean_ms)
    }

    #[getter]
    fn job_time_median_ms(&self) -> PyResult<f64> {
        Ok(self.0.job_time_median_ms)
    }

    #[getter]
    fn job_time_count(&self) -> PyResult<u32> {
        Ok(self.0.job_time_count as u32)
    }

    #[getter]
    fn start_time<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyDateTime>> {
        PyDateTime::from_timestamp(
            py,
            self.0.start_time.timestamp() as f64,
            PyTzInfo::utc(py).ok().as_deref(),
        )
    }

    #[getter]
    fn current_time<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyDateTime>> {
        PyDateTime::from_timestamp(
            py,
            self.0.current_time.timestamp() as f64,
            PyTzInfo::utc(py).ok().as_deref(),
        )
    }

    #[getter]
    fn uptime_seconds(&self) -> PyResult<u64> {
        Ok(self.0.uptime_seconds as u64)
    }

    #[getter]
    fn engine(&self) -> PyResult<String> {
        Ok(self.0.engine.clone())
    }

    #[getter]
    fn version(&self) -> PyResult<String> {
        Ok(self.0.version.clone())
    }

    #[getter]
    fn last_updated<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyDateTime>> {
        PyDateTime::from_timestamp(
            py,
            self.0.last_updated.timestamp() as f64,
            PyTzInfo::utc(py).ok().as_deref(),
        )
    }

    #[getter]
    fn x(&self) -> PyResult<Self> {
        // return a copy that has any children removed. This
        // allows just the health of this single agent to be
        // extracted (x) and printed
        let mut clone = self.clone();
        clone.0.peers.clear();
        Ok(clone)
    }

    fn __str__(&self) -> PyResult<String> {
        Ok(self.0.to_pretty_string())
    }

    fn __repr__(&self) -> PyResult<String> {
        self.__str__()
    }

    fn __copy__(&self) -> PyResult<HealthInfo> {
        Ok(self.clone())
    }

    fn __deepcopy__(&self, _memo: Py<PyAny>) -> PyResult<HealthInfo> {
        Ok(self.clone())
    }

    fn keys(&self) -> PyResult<Vec<String>> {
        Ok(self.0.peers.keys().cloned().collect())
    }

    fn __getitem__(&self, key: &str) -> PyResult<HealthInfo> {
        match self.0.peers.get(key) {
            Some(peer_health) => Ok((**peer_health).clone().into()),
            None => Err(PyErr::new::<PyOSError, _>(format!(
                "No peer health info for key: {}",
                key
            ))),
        }
    }

    fn peers(&self) -> PyResult<HashMap<String, HealthInfo>> {
        let mut result: HashMap<String, HealthInfo> = HashMap::new();
        for (key, value) in &self.0.peers {
            result.insert(key.clone(), (**value).clone().into());
        }
        Ok(result)
    }
}

impl From<mod_health::HealthInfo> for HealthInfo {
    fn from(health_info: mod_health::HealthInfo) -> Self {
        HealthInfo(health_info)
    }
}

///
/// Return type for the health function
///
#[pyclass(module = "openportal")]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Health {
    pub status: String,
    #[serde(default)]
    pub health: Option<HealthInfo>,
}

#[pymethods]
impl Health {
    #[getter]
    fn status(&self) -> PyResult<String> {
        Ok(self.status.clone())
    }

    #[getter]
    fn detail(&self) -> PyResult<Option<HealthInfo>> {
        Ok(self.health.clone())
    }

    fn __str__(&self) -> PyResult<String> {
        let mut s = format!("Health( status: {}", self.status);
        if let Some(ref health) = self.health {
            s.push_str(&format!(", detail:\n{}\n", health.0.to_pretty_string()));
        }
        s.push_str(" )");
        Ok(s)
    }

    fn __repr__(&self) -> PyResult<String> {
        self.__str__()
    }

    fn __copy__(&self) -> PyResult<Health> {
        Ok(self.clone())
    }

    fn __deepcopy__(&self, _memo: Py<PyAny>) -> PyResult<Health> {
        Ok(self.clone())
    }

    fn is_healthy(&self) -> PyResult<bool> {
        Ok(self.status == "ok")
    }

    fn __getitem__(&self, key: &str) -> PyResult<HealthInfo> {
        match &self.health {
            Some(health_info) => match health_info.0.name == key {
                true => Ok(health_info.clone()),
                false => Err(PyErr::new::<PyOSError, _>(format!(
                    "No health information available for key: {}",
                    key
                ))),
            },
            None => Err(PyErr::new::<PyOSError, _>(
                "No health information available",
            )),
        }
    }

    fn keys(&self) -> PyResult<Vec<String>> {
        match &self.health {
            Some(health_info) => Ok(vec![health_info.0.name.clone()]),
            None => Ok(vec![]),
        }
    }
}

///
/// Return the health of the OpenPortal system.
///
#[pyfunction]
fn health() -> PyResult<Health> {
    tracing::debug!("Calling /health");
    match call_get::<Health>("health") {
        Ok(response) => Ok(response),
        Err(e) => Err(PyErr::new::<PyOSError, _>(format!("{:?}", e))),
    }
}

///
/// Return type for the restart function
///
#[pyclass(module = "openportal")]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RestartResponse {
    pub status: String,
    pub message: String,
}

#[pymethods]
impl RestartResponse {
    #[getter]
    fn status(&self) -> PyResult<String> {
        Ok(self.status.clone())
    }

    #[getter]
    fn message(&self) -> PyResult<String> {
        Ok(self.message.clone())
    }

    fn __str__(&self) -> PyResult<String> {
        Ok(format!(
            "RestartResponse( status: {}, message: {} )",
            self.status, self.message
        ))
    }

    fn __repr__(&self) -> PyResult<String> {
        self.__str__()
    }

    fn __copy__(&self) -> PyResult<RestartResponse> {
        Ok(self.clone())
    }

    fn __deepcopy__(&self, _memo: Py<PyAny>) -> PyResult<RestartResponse> {
        Ok(self.clone())
    }

    fn is_ok(&self) -> PyResult<bool> {
        Ok(self.status == "ok")
    }
}

///
/// Restart an agent in the OpenPortal system.
///
/// Parameters:
/// - restart_type: Type of restart ("soft", "hard", etc.)
/// - destination: Dot-separated path to the agent (e.g., "brics.aip2.clusters")
///                Empty string means restart the bridge itself
///
#[pyfunction]
fn restart(restart_type: &str, destination: &str) -> PyResult<RestartResponse> {
    tracing::debug!(
        "Calling /restart with type={}, destination={}",
        restart_type,
        destination
    );

    let params = serde_json::json!({
        "restart_type": restart_type,
        "destination": destination,
    });

    match call_post::<RestartResponse>("restart", params) {
        Ok(response) => Ok(response),
        Err(e) => Err(PyErr::new::<PyOSError, _>(format!("{:?}", e))),
    }
}

///
/// Return type for the run function. This represents the job being
/// run, and provides functions that let you query the status and
/// get the results
///
#[pyclass(module = "openportal")]
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Job(job::Job);

impl From<job::Job> for Job {
    fn from(job: job::Job) -> Self {
        Job(job)
    }
}

#[pymethods]
impl Job {
    fn __str__(&self) -> PyResult<String> {
        Ok(self.0.to_string())
    }

    fn __repr__(&self) -> PyResult<String> {
        self.__str__()
    }

    fn __copy__(&self) -> PyResult<Job> {
        Ok(self.clone())
    }

    fn __deepcopy__(&self, _memo: Py<PyAny>) -> PyResult<Job> {
        Ok(self.clone())
    }

    fn __richcmp__(&self, other: &Job, op: CompareOp) -> PyResult<bool> {
        match op {
            CompareOp::Eq => Ok(self.0 == other.0),
            CompareOp::Ne => Ok(self.0 != other.0),
            _ => Err(PyErr::new::<PyOSError, _>("Invalid comparison operator")),
        }
    }

    fn to_json(&self) -> PyResult<String> {
        self.0
            .to_json()
            .map_err(|e| PyErr::new::<PyOSError, _>(format!("{:?}", e)))
    }

    #[staticmethod]
    fn from_json(json: &str) -> PyResult<Self> {
        match job::Job::from_json(json) {
            Ok(job) => Ok(job.into()),
            Err(e) => Err(PyErr::new::<PyOSError, _>(format!("{:?}", e))),
        }
    }

    #[getter]
    fn id(&self) -> PyResult<Uuid> {
        Ok(self.0.id().into())
    }

    #[getter]
    fn destination(&self) -> PyResult<Destination> {
        Ok(self.0.destination().into())
    }

    #[getter]
    fn instruction(&self) -> PyResult<Instruction> {
        Ok(self.0.instruction().into())
    }

    #[getter]
    fn is_expired(&self) -> PyResult<bool> {
        Ok(self.0.is_expired())
    }

    #[getter]
    fn is_finished(&self) -> PyResult<bool> {
        Ok(self.0.is_finished())
    }

    #[getter]
    fn is_duplicate(&self) -> PyResult<bool> {
        Ok(self.0.is_duplicate())
    }

    #[getter]
    fn state(&self) -> PyResult<Status> {
        Ok(self.0.state().into())
    }

    #[getter]
    fn created<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyDateTime>> {
        PyDateTime::from_timestamp(
            py,
            self.0.created().timestamp() as f64,
            PyTzInfo::utc(py).ok().as_deref(),
        )
    }

    #[getter]
    fn changed<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyDateTime>> {
        PyDateTime::from_timestamp(
            py,
            self.0.changed().timestamp() as f64,
            PyTzInfo::utc(py).ok().as_deref(),
        )
    }

    #[getter]
    fn version(&self) -> PyResult<u64> {
        Ok(self.0.version())
    }

    #[getter]
    fn is_error(&self) -> bool {
        self.0.is_error()
    }

    #[getter]
    fn error_message(&self) -> PyResult<String> {
        match self.0.error_message() {
            Some(message) => Ok(message.clone()),
            None => Ok("".to_string()),
        }
    }

    #[getter]
    fn progress_message(&self) -> PyResult<String> {
        match self.0.progress_message() {
            Some(message) => Ok(message.clone()),
            None => Ok("".to_string()),
        }
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

    #[pyo3(signature = (max_ms=1000))]
    fn wait(&mut self, max_ms: i64) -> PyResult<bool> {
        if max_ms < 0 {
            // wait forever...
            while !self.is_finished()? {
                // sleep for 100ms
                std::thread::sleep(std::time::Duration::from_millis(100));
                self.update()?;
            }
        } else {
            let max_ms: u64 = max_ms as u64;

            let mut total_waited: u64 = 0;

            // check at least 10 times, with a minimum of 1ms and a maximum of 100ms
            let delta: u64 = (max_ms / 10).clamp(1, 100);

            while !self.is_finished()? && total_waited < max_ms {
                // sleep for 100ms
                std::thread::sleep(std::time::Duration::from_millis(delta));
                self.update()?;
                total_waited += delta;
            }
        }

        self.is_finished()
    }

    fn completed(&self, py: Python<'_>, result: Py<PyAny>) -> PyResult<Job> {
        macro_rules! try_extract {
            ($type:ty, $transform:expr) => {
                if let Ok(val) = result.extract::<$type>(py) {
                    let inner_result = $transform(val);
                    return match self.0.completed(inner_result) {
                        Ok(result) => Ok(result.into()),
                        Err(e) => Err(PyErr::new::<PyOSError, _>(format!("{:?}", e))),
                    };
                }
            };
        }

        // Single value extractions
        try_extract!(bool, |v| v);
        try_extract!(String, |v| v);
        try_extract!(UserIdentifier, |v: UserIdentifier| v.0.clone());
        try_extract!(ProjectIdentifier, |v: ProjectIdentifier| v.0.clone());
        try_extract!(PortalIdentifier, |v: PortalIdentifier| v.0.clone());
        try_extract!(UserMapping, |v: UserMapping| v.0.clone());
        try_extract!(ProjectMapping, |v: ProjectMapping| v.0.clone());
        try_extract!(UsageReport, |v: UsageReport| v.0.clone());
        try_extract!(ProjectUsageReport, |v: ProjectUsageReport| v.0.clone());
        try_extract!(Usage, |v: Usage| v.0);
        try_extract!(DateRange, |v: DateRange| v.0.clone());
        try_extract!(ProjectTemplate, |v: ProjectTemplate| v.0.clone());
        try_extract!(ProjectDetails, |v: ProjectDetails| v.0.clone());

        try_extract!(Vec<UserIdentifier>, |v: Vec<UserIdentifier>| {
            v.into_iter().map(|item| item.0.clone()).collect::<Vec<_>>()
        });
        try_extract!(Vec<ProjectIdentifier>, |v: Vec<ProjectIdentifier>| {
            v.into_iter().map(|item| item.0.clone()).collect::<Vec<_>>()
        });
        try_extract!(Vec<PortalIdentifier>, |v: Vec<PortalIdentifier>| {
            v.into_iter().map(|item| item.0.clone()).collect::<Vec<_>>()
        });
        try_extract!(Vec<ProjectMapping>, |v: Vec<ProjectMapping>| {
            v.into_iter().map(|item| item.0.clone()).collect::<Vec<_>>()
        });
        try_extract!(Vec<UserMapping>, |v: Vec<UserMapping>| {
            v.into_iter().map(|item| item.0.clone()).collect::<Vec<_>>()
        });
        try_extract!(Vec<String>, |v| v);
        try_extract!(Vec<Usage>, |v: Vec<Usage>| {
            v.into_iter().map(|item| item.0).collect::<Vec<_>>()
        });

        Err(PyErr::new::<PyOSError, _>("Could not extract result type"))
    }

    fn errored(&self, error: &str) -> PyResult<Job> {
        let result = match self.0.errored(error) {
            Ok(result) => result,
            Err(e) => return Err(PyErr::new::<PyOSError, _>(format!("{:?}", e))),
        };

        Ok(result.into())
    }

    #[getter]
    fn result<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        if !self.is_finished()? {
            return Err(PyErr::new::<PyOSError, _>("Job is not finished"));
        }

        if self.0.is_error() {
            return Err(PyErr::new::<PyOSError, _>(self.error_message()?));
        }

        let result_type = match self.0.result_type() {
            Ok(result_type) => result_type,
            Err(e) => return Err(PyErr::new::<PyOSError, _>(format!("{:?}", e))),
        };

        match result_type.as_str() {
            "String" => {
                let result = match self.0.result::<String>() {
                    Ok(result) => result,
                    Err(e) => return Err(PyErr::new::<PyOSError, _>(format!("{:?}", e))),
                };

                match result {
                    Some(result) => Ok(PyString::new(py, &result).into_any()),
                    None => Ok(py.None().into_bound(py)),
                }
            }
            "bool" => {
                let result = match self.0.result::<bool>() {
                    Ok(result) => result,
                    Err(e) => return Err(PyErr::new::<PyOSError, _>(format!("{:?}", e))),
                };

                match result {
                    Some(result) => Ok((result as u64).into_pyobject(py)?.into_any()),
                    None => Ok(py.None().into_bound(py)),
                }
            }
            "None" => Ok(py.None().into_bound(py)),
            "UserIdentifier" => {
                let result = match self.0.result::<grammar::UserIdentifier>() {
                    Ok(result) => result,
                    Err(e) => return Err(PyErr::new::<PyOSError, _>(format!("{:?}", e))),
                };

                match result {
                    Some(result) => Ok(UserIdentifier::from(result).into_pyobject(py)?.into_any()),
                    None => Ok(py.None().into_bound(py)),
                }
            }
            "ProjectIdentifier" => {
                let result = match self.0.result::<grammar::ProjectIdentifier>() {
                    Ok(result) => result,
                    Err(e) => return Err(PyErr::new::<PyOSError, _>(format!("{:?}", e))),
                };

                match result {
                    Some(result) => Ok(ProjectIdentifier::from(result)
                        .into_pyobject(py)?
                        .into_any()),
                    None => Ok(py.None().into_bound(py)),
                }
            }
            "PortalIdentifier" => {
                let result = match self.0.result::<grammar::PortalIdentifier>() {
                    Ok(result) => result,
                    Err(e) => return Err(PyErr::new::<PyOSError, _>(format!("{:?}", e))),
                };

                match result {
                    Some(result) => {
                        Ok(PortalIdentifier::from(result).into_pyobject(py)?.into_any())
                    }
                    None => Ok(py.None().into_bound(py)),
                }
            }
            "UserMapping" => {
                let result = match self.0.result::<grammar::UserMapping>() {
                    Ok(result) => result,
                    Err(e) => return Err(PyErr::new::<PyOSError, _>(format!("{:?}", e))),
                };

                match result {
                    Some(result) => Ok(UserMapping::from(result).into_pyobject(py)?.into_any()),
                    None => Ok(py.None().into_bound(py)),
                }
            }
            "ProjectMapping" => {
                let result = match self.0.result::<grammar::ProjectMapping>() {
                    Ok(result) => result,
                    Err(e) => return Err(PyErr::new::<PyOSError, _>(format!("{:?}", e))),
                };

                match result {
                    Some(result) => Ok(ProjectMapping::from(result).into_pyobject(py)?.into_any()),
                    None => Ok(py.None().into_bound(py)),
                }
            }
            "UsageReport" => {
                let result = match self.0.result::<usagereport::UsageReport>() {
                    Ok(result) => result,
                    Err(e) => return Err(PyErr::new::<PyOSError, _>(format!("{:?}", e))),
                };

                match result {
                    Some(result) => Ok(UsageReport::from(result).into_pyobject(py)?.into_any()),
                    None => Ok(py.None().into_bound(py)),
                }
            }
            "ProjectUsageReport" => {
                let result = match self.0.result::<usagereport::ProjectUsageReport>() {
                    Ok(result) => result,
                    Err(e) => return Err(PyErr::new::<PyOSError, _>(format!("{:?}", e))),
                };

                match result {
                    Some(result) => Ok(ProjectUsageReport::from(result)
                        .into_pyobject(py)?
                        .into_any()),
                    None => Ok(py.None().into_bound(py)),
                }
            }
            "Usage" => {
                let result = match self.0.result::<usagereport::Usage>() {
                    Ok(result) => result,
                    Err(e) => return Err(PyErr::new::<PyOSError, _>(format!("{:?}", e))),
                };

                match result {
                    Some(result) => Ok(Usage::from(result).into_pyobject(py)?.into_any()),
                    None => Ok(py.None().into_bound(py)),
                }
            }
            "DateRange" => {
                let result = match self.0.result::<grammar::DateRange>() {
                    Ok(result) => result,
                    Err(e) => return Err(PyErr::new::<PyOSError, _>(format!("{:?}", e))),
                };

                match result {
                    Some(result) => Ok(DateRange::from(result).into_pyobject(py)?.into_any()),
                    None => Ok(py.None().into_bound(py)),
                }
            }
            "ProjectDetails" => {
                let result = match self.0.result::<grammar::ProjectDetails>() {
                    Ok(result) => result,
                    Err(e) => return Err(PyErr::new::<PyOSError, _>(format!("{:?}", e))),
                };

                match result {
                    Some(result) => Ok(ProjectDetails::from(result).into_pyobject(py)?.into_any()),
                    None => Ok(py.None().into_bound(py)),
                }
            }
            "ProjectTemplate" => {
                let result = match self.0.result::<grammar::ProjectTemplate>() {
                    Ok(result) => result,
                    Err(e) => return Err(PyErr::new::<PyOSError, _>(format!("{:?}", e))),
                };

                match result {
                    Some(result) => Ok(ProjectTemplate::from(result).into_pyobject(py)?.into_any()),
                    None => Ok(py.None().into_bound(py)),
                }
            }
            "Vec<String>" => {
                let result = match self.0.result::<Vec<String>>() {
                    Ok(result) => result,
                    Err(e) => return Err(PyErr::new::<PyOSError, _>(format!("{:?}", e))),
                };

                match result {
                    Some(result) => {
                        let list = PyList::empty(py);
                        for item in result {
                            list.append(PyString::new(py, &item))?;
                        }
                        Ok(list.into_any())
                    }
                    None => Ok(py.None().into_bound(py)),
                }
            }
            "Vec<UserIdentifier>" => {
                let result = match self.0.result::<Vec<grammar::UserIdentifier>>() {
                    Ok(result) => result,
                    Err(e) => return Err(PyErr::new::<PyOSError, _>(format!("{:?}", e))),
                };

                match result {
                    Some(result) => {
                        let list = PyList::empty(py);
                        for item in result {
                            list.append(UserIdentifier::from(item).into_pyobject(py)?)?;
                        }
                        Ok(list.into_any())
                    }
                    None => Ok(py.None().into_bound(py)),
                }
            }
            "Vec<UserMapping>" => {
                let result = match self.0.result::<Vec<grammar::UserMapping>>() {
                    Ok(result) => result,
                    Err(e) => return Err(PyErr::new::<PyOSError, _>(format!("{:?}", e))),
                };

                match result {
                    Some(result) => {
                        let list = PyList::empty(py);
                        for item in result {
                            list.append(UserMapping::from(item).into_pyobject(py)?)?;
                        }
                        Ok(list.into_any())
                    }
                    None => Ok(py.None().into_bound(py)),
                }
            }
            "Vec<ProjectIdentifier>" => {
                let result = match self.0.result::<Vec<grammar::ProjectIdentifier>>() {
                    Ok(result) => result,
                    Err(e) => return Err(PyErr::new::<PyOSError, _>(format!("{:?}", e))),
                };

                match result {
                    Some(result) => {
                        let list = PyList::empty(py);
                        for item in result {
                            list.append(ProjectIdentifier::from(item).into_pyobject(py)?)?;
                        }
                        Ok(list.into_any())
                    }
                    None => Ok(py.None().into_bound(py)),
                }
            }
            "Vec<ProjectMapping>" => {
                let result = match self.0.result::<Vec<grammar::ProjectMapping>>() {
                    Ok(result) => result,
                    Err(e) => return Err(PyErr::new::<PyOSError, _>(format!("{:?}", e))),
                };

                match result {
                    Some(result) => {
                        let list = PyList::empty(py);
                        for item in result {
                            list.append(ProjectMapping::from(item).into_pyobject(py)?)?;
                        }
                        Ok(list.into_any())
                    }
                    None => Ok(py.None().into_bound(py)),
                }
            }
            "Vec<PortalIdentifier>" => {
                let result = match self.0.result::<Vec<grammar::PortalIdentifier>>() {
                    Ok(result) => result,
                    Err(e) => return Err(PyErr::new::<PyOSError, _>(format!("{:?}", e))),
                };

                match result {
                    Some(result) => {
                        let list = PyList::empty(py);
                        for item in result {
                            list.append(PortalIdentifier::from(item).into_pyobject(py)?)?;
                        }
                        Ok(list.into_any())
                    }
                    None => Ok(py.None().into_bound(py)),
                }
            }
            _ => Err(PyErr::new::<PyOSError, _>(format!(
                "Unknown result type: {}",
                result_type
            ))),
        }
    }
}

///
/// Wrappers for the publicly exposed data types
///

#[pyclass(module = "openportal")]
#[derive(Debug, Clone, Serialize, Deserialize)]
struct DateRange(grammar::DateRange);

#[pymethods]
impl DateRange {
    #[new]
    fn new(start_date: chrono::NaiveDate, end_date: chrono::NaiveDate) -> PyResult<Self> {
        Ok(grammar::DateRange::from_chrono(&start_date, &end_date).into())
    }

    #[staticmethod]
    fn parse(date_range: String) -> PyResult<Self> {
        match grammar::DateRange::parse(&date_range) {
            Ok(date_range) => Ok(date_range.into()),
            Err(e) => Err(PyErr::new::<PyOSError, _>(format!("{:?}", e))),
        }
    }

    #[staticmethod]
    fn yesterday() -> PyResult<Self> {
        Ok(grammar::Date::yesterday().day().into())
    }

    #[staticmethod]
    fn today() -> PyResult<Self> {
        Ok(grammar::Date::today().day().into())
    }

    #[staticmethod]
    fn tomorrow() -> PyResult<Self> {
        Ok(grammar::Date::tomorrow().day().into())
    }

    #[staticmethod]
    fn last_month() -> PyResult<Self> {
        Ok(grammar::Date::today().prev_month().into())
    }

    #[staticmethod]
    fn next_month() -> PyResult<Self> {
        Ok(grammar::Date::today().next_month().into())
    }

    #[staticmethod]
    fn this_month() -> PyResult<Self> {
        Ok(grammar::Date::today().month().into())
    }

    #[staticmethod]
    fn this_week() -> PyResult<Self> {
        Ok(grammar::Date::today().week().into())
    }

    #[staticmethod]
    fn last_week() -> PyResult<Self> {
        Ok(grammar::Date::today().prev_week().into())
    }

    #[staticmethod]
    fn next_week() -> PyResult<Self> {
        Ok(grammar::Date::today().next_week().into())
    }

    #[staticmethod]
    fn last_year() -> PyResult<Self> {
        Ok(grammar::Date::today().prev_year().into())
    }

    #[staticmethod]
    fn next_year() -> PyResult<Self> {
        Ok(grammar::Date::today().next_year().into())
    }

    #[staticmethod]
    fn this_year() -> PyResult<Self> {
        Ok(grammar::Date::today().year().into())
    }

    #[staticmethod]
    fn week(date: chrono::NaiveDate) -> PyResult<Self> {
        Ok(grammar::Date::from_chrono(&date).week().into())
    }

    #[staticmethod]
    fn month(date: chrono::NaiveDate) -> PyResult<Self> {
        Ok(grammar::Date::from_chrono(&date).month().into())
    }

    #[staticmethod]
    fn year(date: chrono::NaiveDate) -> PyResult<Self> {
        Ok(grammar::Date::from_chrono(&date).year().into())
    }

    #[getter]
    fn start_date<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyDate>> {
        self.0.start_date().to_chrono().into_pyobject(py)
    }

    #[getter]
    fn end_date<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyDate>> {
        self.0.end_date().to_chrono().into_pyobject(py)
    }

    #[getter]
    fn start_time<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyDateTime>> {
        self.0.start_time().into_pyobject(py)
    }

    #[getter]
    fn end_time<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyDateTime>> {
        self.0.end_time().into_pyobject(py)
    }

    #[getter]
    fn days<'py>(&self, py: Python<'py>) -> PyResult<Vec<Bound<'py, PyDate>>> {
        let mut days = Vec::new();

        for day in self.0.days() {
            days.push(PyDate::from_timestamp(py, day.timestamp())?);
        }

        Ok(days)
    }

    #[getter]
    fn months(&self) -> PyResult<Vec<DateRange>> {
        let mut months = Vec::new();

        for month in self.0.months() {
            months.push(month.into());
        }

        Ok(months)
    }

    #[getter]
    fn weeks(&self) -> PyResult<Vec<DateRange>> {
        let mut weeks = Vec::new();

        for week in self.0.weeks() {
            weeks.push(week.into());
        }

        Ok(weeks)
    }

    #[getter]
    fn years(&self) -> PyResult<Vec<DateRange>> {
        let mut years = Vec::new();

        for year in self.0.years() {
            years.push(year.into());
        }

        Ok(years)
    }

    fn __str__(&self) -> PyResult<String> {
        Ok(self.0.to_string())
    }

    fn __repr__(&self) -> PyResult<String> {
        self.__str__()
    }

    fn __copy__(&self) -> PyResult<DateRange> {
        Ok(self.clone())
    }

    fn __deepcopy__(&self, _memo: Py<PyAny>) -> PyResult<DateRange> {
        Ok(self.clone())
    }

    fn __richcmp__(&self, other: &DateRange, op: CompareOp) -> PyResult<bool> {
        match op {
            CompareOp::Eq => Ok(self.0 == other.0),
            CompareOp::Ne => Ok(self.0 != other.0),
            _ => Err(PyErr::new::<PyOSError, _>("Invalid comparison operator")),
        }
    }
}

impl From<grammar::DateRange> for DateRange {
    fn from(date_range: grammar::DateRange) -> Self {
        DateRange(date_range)
    }
}

#[pyclass(module = "openportal")]
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Node(grammar::Node);

#[pymethods]
impl Node {
    #[new]
    fn new() -> PyResult<Self> {
        Ok(grammar::Node::new().into())
    }

    #[staticmethod]
    fn construct(cpus: u32, cores_per_cpu: u32, gpus: u32, memory_mb: u32) -> PyResult<Self> {
        Ok(grammar::Node::construct(cpus, cores_per_cpu, gpus, memory_mb).into())
    }

    #[getter]
    fn cpus(&self) -> PyResult<u32> {
        Ok(self.0.cpus())
    }

    #[getter]
    fn cores_per_cpu(&self) -> PyResult<u32> {
        Ok(self.0.cores_per_cpu())
    }

    #[getter]
    fn cores(&self) -> PyResult<u32> {
        Ok(self.0.cores())
    }

    #[getter]
    fn gpus(&self) -> PyResult<u32> {
        Ok(self.0.gpus())
    }

    #[getter]
    fn memory_mb(&self) -> PyResult<u32> {
        Ok(self.0.memory_mb())
    }

    #[getter]
    fn memory_gb(&self) -> PyResult<f64> {
        Ok(self.0.memory_gb())
    }

    #[setter]
    fn set_cpus(&mut self, cpus: u32) -> PyResult<()> {
        self.0.set_cpus(cpus);
        Ok(())
    }

    #[setter]
    fn set_cores_per_cpu(&mut self, cores_per_cpu: u32) -> PyResult<()> {
        self.0.set_cores_per_cpu(cores_per_cpu);
        Ok(())
    }

    #[setter]
    fn set_gpus(&mut self, gpus: u32) -> PyResult<()> {
        self.0.set_gpus(gpus);
        Ok(())
    }

    #[setter]
    fn set_memory_mb(&mut self, memory_mb: u32) -> PyResult<()> {
        self.0.set_memory_mb(memory_mb);
        Ok(())
    }

    fn __str__(&self) -> PyResult<String> {
        Ok(self.0.to_string())
    }

    fn __repr__(&self) -> PyResult<String> {
        self.__str__()
    }

    fn __copy__(&self) -> PyResult<Node> {
        Ok(self.clone())
    }

    fn __deepcopy__(&self, _memo: Py<PyAny>) -> PyResult<Node> {
        Ok(self.clone())
    }
}

impl From<grammar::Node> for Node {
    fn from(node: grammar::Node) -> Self {
        Node(node)
    }
}

#[pyclass(module = "openportal")]
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Allocation(grammar::Allocation);

#[pymethods]
impl Allocation {
    #[new]
    fn new() -> PyResult<Self> {
        Ok(grammar::Allocation::new().into())
    }

    #[staticmethod]
    fn parse(allocation: String) -> PyResult<Self> {
        match grammar::Allocation::parse(&allocation) {
            Ok(allocation) => Ok(allocation.into()),
            Err(e) => Err(PyErr::new::<PyOSError, _>(format!("{:?}", e))),
        }
    }

    #[staticmethod]
    fn from_size_and_units(size: f64, units: &str) -> PyResult<Self> {
        match grammar::Allocation::from_size_and_units(size, units) {
            Ok(allocation) => Ok(allocation.into()),
            Err(e) => Err(PyErr::new::<PyOSError, _>(format!("{:?}", e))),
        }
    }

    #[staticmethod]
    fn from_string(allocation: &str) -> PyResult<Allocation> {
        match grammar::Allocation::parse(allocation) {
            Ok(allocation) => Ok(allocation.into()),
            Err(e) => Err(PyErr::new::<PyOSError, _>(format!("{:?}", e))),
        }
    }

    fn to_string(&self) -> PyResult<String> {
        Ok(self.0.to_string())
    }

    #[getter]
    fn size(&self) -> PyResult<Option<f64>> {
        Ok(self.0.size())
    }

    #[getter]
    fn units(&self) -> PyResult<Option<String>> {
        Ok(self.0.units())
    }

    #[staticmethod]
    fn canonicalize(units: &str) -> PyResult<String> {
        Ok(grammar::Allocation::canonicalize(units))
    }

    #[getter]
    fn is_empty(&self) -> PyResult<bool> {
        Ok(self.0.is_empty())
    }

    #[getter]
    fn is_node_hours(&self) -> PyResult<bool> {
        Ok(self.0.is_node_hours())
    }

    #[getter]
    fn is_core_hours(&self) -> PyResult<bool> {
        Ok(self.0.is_core_hours())
    }

    #[getter]
    fn is_gpu_hours(&self) -> PyResult<bool> {
        Ok(self.0.is_gpu_hours())
    }

    #[getter]
    fn is_cpu_hours(&self) -> PyResult<bool> {
        Ok(self.0.is_cpu_hours())
    }

    #[getter]
    fn is_gb_hours(&self) -> PyResult<bool> {
        Ok(self.0.is_gb_hours())
    }

    fn to_node_hours(&self, node: &Node) -> PyResult<Usage> {
        match self.0.to_node_hours(&node.0) {
            Ok(usage) => Ok(usage.into()),
            Err(e) => Err(PyErr::new::<PyOSError, _>(format!("{:?}", e))),
        }
    }

    fn to_core_hours(&self, node: &Node) -> PyResult<Usage> {
        match self.0.to_core_hours(&node.0) {
            Ok(usage) => Ok(usage.into()),
            Err(e) => Err(PyErr::new::<PyOSError, _>(format!("{:?}", e))),
        }
    }

    fn to_gpu_hours(&self, node: &Node) -> PyResult<Usage> {
        match self.0.to_gpu_hours(&node.0) {
            Ok(usage) => Ok(usage.into()),
            Err(e) => Err(PyErr::new::<PyOSError, _>(format!("{:?}", e))),
        }
    }

    fn to_cpu_hours(&self, node: &Node) -> PyResult<Usage> {
        match self.0.to_cpu_hours(&node.0) {
            Ok(usage) => Ok(usage.into()),
            Err(e) => Err(PyErr::new::<PyOSError, _>(format!("{:?}", e))),
        }
    }

    fn to_gb_hours(&self, node: &Node) -> PyResult<Usage> {
        match self.0.to_gb_hours(&node.0) {
            Ok(usage) => Ok(usage.into()),
            Err(e) => Err(PyErr::new::<PyOSError, _>(format!("{:?}", e))),
        }
    }

    #[staticmethod]
    fn from_node_hours(usage: &Usage) -> PyResult<Self> {
        match grammar::Allocation::from_node_hours(&usage.0) {
            Ok(allocation) => Ok(allocation.into()),
            Err(e) => Err(PyErr::new::<PyOSError, _>(format!("{:?}", e))),
        }
    }

    #[staticmethod]
    fn from_cpu_hours(usage: &Usage, node: &Node) -> PyResult<Self> {
        match grammar::Allocation::from_cpu_hours(&usage.0, &node.0) {
            Ok(allocation) => Ok(allocation.into()),
            Err(e) => Err(PyErr::new::<PyOSError, _>(format!("{:?}", e))),
        }
    }

    #[staticmethod]
    fn from_core_hours(usage: &Usage, node: &Node) -> PyResult<Self> {
        match grammar::Allocation::from_core_hours(&usage.0, &node.0) {
            Ok(allocation) => Ok(allocation.into()),
            Err(e) => Err(PyErr::new::<PyOSError, _>(format!("{:?}", e))),
        }
    }

    #[staticmethod]
    fn from_gpu_hours(usage: &Usage, node: &Node) -> PyResult<Self> {
        match grammar::Allocation::from_gpu_hours(&usage.0, &node.0) {
            Ok(allocation) => Ok(allocation.into()),
            Err(e) => Err(PyErr::new::<PyOSError, _>(format!("{:?}", e))),
        }
    }

    #[staticmethod]
    fn from_gb_hours(usage: &Usage, node: &Node) -> PyResult<Self> {
        match grammar::Allocation::from_gb_hours(&usage.0, &node.0) {
            Ok(allocation) => Ok(allocation.into()),
            Err(e) => Err(PyErr::new::<PyOSError, _>(format!("{:?}", e))),
        }
    }

    fn __str__(&self) -> PyResult<String> {
        Ok(self.0.to_string())
    }

    fn __repr__(&self) -> PyResult<String> {
        self.__str__()
    }

    fn __copy__(&self) -> PyResult<Allocation> {
        Ok(self.clone())
    }

    fn __deepcopy__(&self, _memo: Py<PyAny>) -> PyResult<Allocation> {
        Ok(self.clone())
    }

    fn __richcmp__(&self, other: &Allocation, op: CompareOp) -> PyResult<bool> {
        match op {
            CompareOp::Eq => Ok(self.0 == other.0),
            CompareOp::Ne => Ok(self.0 != other.0),
            _ => Err(PyErr::new::<PyOSError, _>("Invalid comparison operator")),
        }
    }
}

impl From<grammar::Allocation> for Allocation {
    fn from(allocation: grammar::Allocation) -> Self {
        Allocation(allocation)
    }
}

#[pyclass(module = "openportal")]
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Usage(usagereport::Usage);

#[pymethods]
impl Usage {
    #[new]
    fn new(usage: u64) -> PyResult<Self> {
        Ok(Self(usagereport::Usage::new(usage)))
    }

    #[staticmethod]
    fn from_seconds(seconds: u64) -> PyResult<Self> {
        Ok(Self(usagereport::Usage::from_seconds(seconds)))
    }

    #[staticmethod]
    fn from_minutes(minutes: f64) -> PyResult<Self> {
        Ok(Self(usagereport::Usage::from_minutes(minutes)))
    }

    #[staticmethod]
    fn from_hours(hours: f64) -> PyResult<Self> {
        Ok(Self(usagereport::Usage::from_hours(hours)))
    }

    #[staticmethod]
    fn from_days(days: f64) -> PyResult<Self> {
        Ok(Self(usagereport::Usage::from_days(days)))
    }

    #[staticmethod]
    fn from_weeks(weeks: f64) -> PyResult<Self> {
        Ok(Self(usagereport::Usage::from_weeks(weeks)))
    }

    #[staticmethod]
    fn from_months(months: f64) -> PyResult<Self> {
        Ok(Self(usagereport::Usage::from_months(months)))
    }

    #[staticmethod]
    fn from_years(years: f64) -> PyResult<Self> {
        Ok(Self(usagereport::Usage::from_years(years)))
    }

    #[getter]
    fn seconds(&self) -> PyResult<u64> {
        Ok(self.0.seconds())
    }

    #[getter]
    fn minutes(&self) -> PyResult<f64> {
        Ok(self.0.minutes())
    }

    #[getter]
    fn hours(&self) -> PyResult<f64> {
        Ok(self.0.hours())
    }

    #[getter]
    fn days(&self) -> PyResult<f64> {
        Ok(self.0.days())
    }

    #[getter]
    fn weeks(&self) -> PyResult<f64> {
        Ok(self.0.weeks())
    }

    #[getter]
    fn months(&self) -> PyResult<f64> {
        Ok(self.0.months())
    }

    #[getter]
    fn years(&self) -> PyResult<f64> {
        Ok(self.0.years())
    }

    fn __str__(&self) -> PyResult<String> {
        Ok(self.0.to_string())
    }

    fn __repr__(&self) -> PyResult<String> {
        self.__str__()
    }

    fn __copy__(&self) -> PyResult<Usage> {
        Ok(self.clone())
    }

    fn __deepcopy__(&self, _memo: Py<PyAny>) -> PyResult<Usage> {
        Ok(self.clone())
    }

    fn __add__(&self, other: &Usage) -> PyResult<Self> {
        Ok(Self(self.0 + other.0))
    }

    fn __sub__(&self, other: &Usage) -> PyResult<Self> {
        Ok(Self(self.0 - other.0))
    }

    fn __mul__(&self, other: f64) -> PyResult<Self> {
        Ok(Self(self.0 * other))
    }

    fn __div__(&self, other: f64) -> PyResult<Self> {
        Ok(Self(self.0 / other))
    }

    fn __rmul__(&self, other: f64) -> PyResult<Self> {
        Ok(Self(self.0 * other))
    }

    fn __iadd__(&mut self, other: &Usage) -> PyResult<()> {
        self.0 += other.0;
        Ok(())
    }

    fn __isub__(&mut self, other: &Usage) -> PyResult<()> {
        self.0 -= other.0;
        Ok(())
    }

    fn __imul__(&mut self, other: f64) -> PyResult<()> {
        self.0 *= other;
        Ok(())
    }

    fn __idiv__(&mut self, other: f64) -> PyResult<()> {
        self.0 /= other;
        Ok(())
    }
}

impl From<usagereport::Usage> for Usage {
    fn from(usage: usagereport::Usage) -> Self {
        Usage(usage)
    }
}

#[pyclass(module = "openportal")]
#[derive(Debug, Clone, Serialize, Deserialize)]
struct UsageReport(usagereport::UsageReport);

#[pymethods]
impl UsageReport {
    #[new]
    fn new(portal: &PortalIdentifier) -> PyResult<Self> {
        Ok(Self(usagereport::UsageReport::new(&portal.0)))
    }

    fn to_json(&self) -> PyResult<String> {
        match self.0.to_json() {
            Ok(json) => Ok(json),
            Err(e) => Err(PyErr::new::<PyOSError, _>(format!("{:?}", e))),
        }
    }

    #[staticmethod]
    fn from_json(json: &str) -> PyResult<Self> {
        match usagereport::UsageReport::from_json(json) {
            Ok(report) => Ok(report.into()),
            Err(e) => Err(PyErr::new::<PyOSError, _>(format!("{:?}", e))),
        }
    }

    fn __str__(&self) -> PyResult<String> {
        Ok(self.0.to_string())
    }

    fn __repr__(&self) -> PyResult<String> {
        self.__str__()
    }

    fn __copy__(&self) -> PyResult<UsageReport> {
        Ok(self.clone())
    }

    fn __deepcopy__(&self, _memo: Py<PyAny>) -> PyResult<UsageReport> {
        Ok(self.clone())
    }

    fn __add__(&self, other: &UsageReport) -> PyResult<Self> {
        Ok(Self(self.0.clone() + other.0.clone()))
    }

    fn __iadd__(&mut self, other: &UsageReport) -> PyResult<()> {
        self.0 += other.0.clone();
        Ok(())
    }

    fn __mul__(&self, factor: f64) -> PyResult<Self> {
        Ok(Self(self.0.clone() * factor))
    }

    fn __div__(&self, divisor: f64) -> PyResult<Self> {
        Ok(Self(self.0.clone() / divisor))
    }

    fn __rmul__(&self, other: f64) -> PyResult<Self> {
        Ok(Self(self.0.clone() * other))
    }

    fn __imul__(&mut self, other: f64) -> PyResult<()> {
        self.0 *= other;
        Ok(())
    }

    fn __idiv__(&mut self, other: f64) -> PyResult<()> {
        self.0 /= other;
        Ok(())
    }

    #[getter]
    fn portal(&self) -> PyResult<PortalIdentifier> {
        Ok(self.0.portal().clone().into())
    }

    #[getter]
    fn projects(&self) -> PyResult<Vec<ProjectIdentifier>> {
        Ok(self.0.projects().iter().map(|p| p.clone().into()).collect())
    }

    fn get_report(&self, project: &ProjectIdentifier) -> PyResult<ProjectUsageReport> {
        Ok(self.0.get_report(&project.0).into())
    }

    #[getter]
    fn total_usage(&self) -> PyResult<Usage> {
        Ok(self.0.total_usage().into())
    }

    #[staticmethod]
    fn combine(reports: Py<PyAny>, py: Python) -> PyResult<Self> {
        let reports: Vec<UsageReport> = reports.extract(py)?;

        let reports: Vec<usagereport::UsageReport> = reports.iter().map(|r| r.0.clone()).collect();

        match usagereport::UsageReport::combine(&reports) {
            Ok(report) => Ok(report.into()),
            Err(e) => Err(PyErr::new::<PyOSError, _>(format!("{:?}", e))),
        }
    }
}

impl From<usagereport::UsageReport> for UsageReport {
    fn from(usage_report: usagereport::UsageReport) -> Self {
        UsageReport(usage_report)
    }
}

#[pyclass(module = "openportal")]
#[derive(Debug, Clone, Serialize, Deserialize)]
struct ProjectUsageReport(usagereport::ProjectUsageReport);

#[pymethods]
impl ProjectUsageReport {
    #[new]
    fn new(project: &ProjectIdentifier) -> PyResult<Self> {
        Ok(Self(usagereport::ProjectUsageReport::new(&project.0)))
    }

    fn to_json(&self) -> PyResult<String> {
        match self.0.to_json() {
            Ok(json) => Ok(json),
            Err(e) => Err(PyErr::new::<PyOSError, _>(format!("{:?}", e))),
        }
    }

    #[staticmethod]
    fn from_json(json: &str) -> PyResult<Self> {
        match usagereport::ProjectUsageReport::from_json(json) {
            Ok(report) => Ok(report.into()),
            Err(e) => Err(PyErr::new::<PyOSError, _>(format!("{:?}", e))),
        }
    }

    fn __str__(&self) -> PyResult<String> {
        Ok(self.0.to_string())
    }

    fn __repr__(&self) -> PyResult<String> {
        self.__str__()
    }

    fn __copy__(&self) -> PyResult<ProjectUsageReport> {
        Ok(self.clone())
    }

    fn __deepcopy__(&self, _memo: Py<PyAny>) -> PyResult<ProjectUsageReport> {
        Ok(self.clone())
    }

    fn __add__(&self, other: &ProjectUsageReport) -> PyResult<Self> {
        Ok(Self(self.0.clone() + other.0.clone()))
    }

    fn __iadd__(&mut self, other: &ProjectUsageReport) -> PyResult<()> {
        self.0 += other.0.clone();
        Ok(())
    }

    fn __mul__(&self, factor: f64) -> PyResult<Self> {
        Ok(Self(self.0.clone() * factor))
    }

    fn __div__(&self, divisor: f64) -> PyResult<Self> {
        Ok(Self(self.0.clone() / divisor))
    }

    fn __rmul__(&self, other: f64) -> PyResult<Self> {
        Ok(Self(self.0.clone() * other))
    }

    fn __imul__(&mut self, other: f64) -> PyResult<()> {
        self.0 *= other;
        Ok(())
    }

    fn __idiv__(&mut self, other: f64) -> PyResult<()> {
        self.0 /= other;
        Ok(())
    }

    #[getter]
    fn dates<'py>(&self, py: Python<'py>) -> PyResult<Vec<Bound<'py, PyDate>>> {
        let mut dates = Vec::new();

        for date in self.0.dates() {
            dates.push(PyDate::from_timestamp(py, date.timestamp())?);
        }

        Ok(dates)
    }

    #[getter]
    fn project(&self) -> PyResult<ProjectIdentifier> {
        Ok(self.0.project().clone().into())
    }

    #[getter]
    fn portal(&self) -> PyResult<PortalIdentifier> {
        Ok(self.0.portal().clone().into())
    }

    #[getter]
    fn users(&self) -> PyResult<Vec<UserIdentifier>> {
        Ok(self.0.users().iter().map(|u| u.clone().into()).collect())
    }

    #[getter]
    fn unmapped_users(&self) -> PyResult<Vec<String>> {
        Ok(self.0.unmapped_users())
    }

    #[getter]
    fn total_usage(&self) -> PyResult<Usage> {
        Ok(self.0.total_usage().into())
    }

    #[getter]
    fn num_jobs(&self) -> PyResult<u64> {
        Ok(self.0.num_jobs())
    }

    #[getter]
    fn unmapped_usage(&self) -> PyResult<Usage> {
        Ok(self.0.unmapped_usage().into())
    }

    #[getter]
    fn is_complete(&self) -> PyResult<bool> {
        Ok(self.0.is_complete())
    }

    fn usage(&self, user: &UserIdentifier) -> PyResult<Usage> {
        Ok(self.0.usage(&user.0).into())
    }

    fn get_report(&self, date: chrono::NaiveDate) -> PyResult<ProjectUsageReport> {
        Ok(self.0.get_report(&grammar::Date::from_chrono(&date)).into())
    }

    #[staticmethod]
    fn combine(reports: Py<PyAny>, py: Python) -> PyResult<Self> {
        let reports: Vec<ProjectUsageReport> = reports.extract(py)?;

        let reports: Vec<usagereport::ProjectUsageReport> =
            reports.iter().map(|r| r.0.clone()).collect();

        match usagereport::ProjectUsageReport::combine(&reports) {
            Ok(report) => Ok(report.into()),
            Err(e) => Err(PyErr::new::<PyOSError, _>(format!("{:?}", e))),
        }
    }

    fn add_mapping(&mut self, user: &UserMapping) -> PyResult<()> {
        match self.0.add_mapping(&user.0) {
            Ok(()) => Ok(()),
            Err(e) => Err(PyErr::new::<PyOSError, _>(format!("{:?}", e))),
        }
    }

    fn add_mappings(&mut self, users: Py<PyAny>, py: Python) -> PyResult<()> {
        let mappings: Vec<UserMapping> = users.extract(py)?;
        let mappings: Vec<grammar::UserMapping> = mappings.iter().map(|m| m.0.clone()).collect();

        match self.0.add_mappings(&mappings) {
            Ok(()) => Ok(()),
            Err(e) => Err(PyErr::new::<PyOSError, _>(format!("{:?}", e))),
        }
    }

    fn set_report(
        &mut self,
        date: chrono::NaiveDate,
        report: &DailyProjectUsageReport,
    ) -> PyResult<()> {
        self.0
            .set_report(&grammar::Date::from_chrono(&date), &report.0);
        Ok(())
    }

    fn add_report(
        &mut self,
        date: chrono::NaiveDate,
        report: &DailyProjectUsageReport,
    ) -> PyResult<()> {
        self.0
            .add_report(&grammar::Date::from_chrono(&date), &report.0);
        Ok(())
    }

    fn set_complete(&mut self) -> PyResult<()> {
        self.0.set_complete();
        Ok(())
    }

    fn set_day_complete(&mut self, date: chrono::NaiveDate) -> PyResult<()> {
        self.0.set_day_complete(&grammar::Date::from_chrono(&date));
        Ok(())
    }

    fn to_usage_report(&self) -> UsageReport {
        self.0.to_usage_report().into()
    }
}

impl From<usagereport::ProjectUsageReport> for ProjectUsageReport {
    fn from(project_usage_report: usagereport::ProjectUsageReport) -> Self {
        ProjectUsageReport(project_usage_report)
    }
}

#[pyclass(module = "openportal")]
#[derive(Debug, Clone, Serialize, Deserialize)]
struct DailyProjectUsageReport(usagereport::DailyProjectUsageReport);

#[pymethods]
impl DailyProjectUsageReport {
    #[new]
    fn new() -> PyResult<Self> {
        Ok(Self(usagereport::DailyProjectUsageReport::default()))
    }

    fn __str__(&self) -> PyResult<String> {
        Ok(self.0.to_string())
    }

    fn __repr__(&self) -> PyResult<String> {
        self.__str__()
    }

    fn __copy__(&self) -> PyResult<DailyProjectUsageReport> {
        Ok(self.clone())
    }

    fn __deepcopy__(&self, _memo: Py<PyAny>) -> PyResult<DailyProjectUsageReport> {
        Ok(self.clone())
    }

    fn __add__(&self, other: &DailyProjectUsageReport) -> PyResult<Self> {
        Ok(Self(self.0.clone() + other.0.clone()))
    }

    fn __iadd__(&mut self, other: &DailyProjectUsageReport) -> PyResult<()> {
        self.0 += other.0.clone();
        Ok(())
    }

    fn __mul__(&self, factor: f64) -> PyResult<Self> {
        Ok(Self(self.0.clone() * factor))
    }

    fn __div__(&self, divisor: f64) -> PyResult<Self> {
        Ok(Self(self.0.clone() / divisor))
    }

    fn __rmul__(&self, other: f64) -> PyResult<Self> {
        Ok(Self(self.0.clone() * other))
    }

    fn __imul__(&mut self, other: f64) -> PyResult<()> {
        self.0 *= other;
        Ok(())
    }

    fn __idiv__(&mut self, other: f64) -> PyResult<()> {
        self.0 /= other;
        Ok(())
    }

    fn usage(&self, user: &str) -> PyResult<Usage> {
        Ok(self.0.usage(user).into())
    }

    #[getter]
    fn num_jobs(&self) -> PyResult<u64> {
        Ok(self.0.num_jobs())
    }

    fn local_users(&self) -> PyResult<Vec<String>> {
        Ok(self.0.local_users().clone())
    }

    #[getter]
    fn total_usage(&self) -> PyResult<Usage> {
        Ok(self.0.total_usage().into())
    }

    fn add_usage(&mut self, user: &str, usage: &Usage) -> PyResult<()> {
        self.0.add_usage(user, usage.0);
        Ok(())
    }

    fn add_unattributed_usage(&mut self, usage: &Usage) -> PyResult<()> {
        self.0.add_unattributed_usage(usage.0);
        Ok(())
    }

    fn set_usage(&mut self, user: &str, usage: &Usage) -> PyResult<()> {
        self.0.set_usage(user, usage.0);
        Ok(())
    }

    fn set_unattributed_usage(&mut self, usage: &Usage) -> PyResult<()> {
        self.0.set_unattributed_usage(usage.0);
        Ok(())
    }

    fn set_complete(&mut self) -> PyResult<()> {
        self.0.set_complete();
        Ok(())
    }

    #[getter]
    fn is_complete(&self) -> PyResult<bool> {
        Ok(self.0.is_complete())
    }
}

impl From<usagereport::DailyProjectUsageReport> for DailyProjectUsageReport {
    fn from(daily_project_usage_report: usagereport::DailyProjectUsageReport) -> Self {
        DailyProjectUsageReport(daily_project_usage_report)
    }
}

#[pyclass(module = "openportal")]
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Destination(destination::Destination);

#[pymethods]
impl Destination {
    #[new]
    fn new(destination: &str) -> PyResult<Self> {
        match destination::Destination::parse(destination) {
            Ok(destination) => Ok(Self(destination)),
            Err(e) => Err(PyErr::new::<PyOSError, _>(format!("{:?}", e))),
        }
    }

    #[getter]
    fn agents(&self) -> PyResult<Vec<String>> {
        Ok(self.0.agents().clone())
    }

    fn __str__(&self) -> PyResult<String> {
        Ok(self.0.to_string())
    }

    fn __repr__(&self) -> PyResult<String> {
        self.__str__()
    }

    fn __copy__(&self) -> PyResult<Destination> {
        Ok(self.clone())
    }

    fn __deepcopy__(&self, _memo: Py<PyAny>) -> PyResult<Destination> {
        Ok(self.clone())
    }

    fn __richcmp__(&self, other: &Destination, op: CompareOp) -> PyResult<bool> {
        match op {
            CompareOp::Eq => Ok(self.0 == other.0),
            CompareOp::Ne => Ok(self.0 != other.0),
            _ => Err(PyErr::new::<PyOSError, _>("Invalid comparison operator")),
        }
    }
}

impl From<destination::Destination> for Destination {
    fn from(destination: destination::Destination) -> Self {
        Destination(destination)
    }
}

#[pyclass(module = "openportal")]
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Instruction(grammar::Instruction);

#[pymethods]
impl Instruction {
    #[new]
    fn new(instruction: &str) -> PyResult<Self> {
        match grammar::Instruction::parse(instruction) {
            Ok(instruction) => Ok(Self(instruction)),
            Err(e) => Err(PyErr::new::<PyOSError, _>(format!("{:?}", e))),
        }
    }

    fn __str__(&self) -> PyResult<String> {
        Ok(self.0.to_string())
    }

    fn __repr__(&self) -> PyResult<String> {
        self.__str__()
    }

    fn __copy__(&self) -> PyResult<Instruction> {
        Ok(self.clone())
    }

    fn __deepcopy__(&self, _memo: Py<PyAny>) -> PyResult<Instruction> {
        Ok(self.clone())
    }

    fn __richcmp__(&self, other: &Instruction, op: CompareOp) -> PyResult<bool> {
        match op {
            CompareOp::Eq => Ok(self.0 == other.0),
            CompareOp::Ne => Ok(self.0 != other.0),
            _ => Err(PyErr::new::<PyOSError, _>("Invalid comparison operator")),
        }
    }

    #[getter]
    fn command(&self) -> PyResult<String> {
        Ok(self.0.command())
    }

    #[getter]
    fn arguments(&self) -> PyResult<Vec<String>> {
        Ok(self.0.arguments().clone())
    }
}

impl From<grammar::Instruction> for Instruction {
    fn from(instruction: grammar::Instruction) -> Self {
        Instruction(instruction)
    }
}

#[pyclass(module = "openportal")]
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Uuid(uuid::Uuid);

#[pymethods]
impl Uuid {
    #[new]
    fn new(uuid: &str) -> PyResult<Self> {
        match uuid::Uuid::parse_str(uuid) {
            Ok(uuid) => Ok(Self(uuid)),
            Err(e) => Err(PyErr::new::<PyOSError, _>(format!("{:?}", e))),
        }
    }

    fn __str__(&self) -> PyResult<String> {
        Ok(self.0.to_string())
    }

    fn __repr__(&self) -> PyResult<String> {
        self.__str__()
    }

    fn __copy__(&self) -> PyResult<Uuid> {
        Ok(self.clone())
    }

    fn __deepcopy__(&self, _memo: Py<PyAny>) -> PyResult<Uuid> {
        Ok(self.clone())
    }

    fn __richcmp__(&self, other: &Uuid, op: CompareOp) -> PyResult<bool> {
        match op {
            CompareOp::Eq => Ok(self.0 == other.0),
            CompareOp::Ne => Ok(self.0 != other.0),
            _ => Err(PyErr::new::<PyOSError, _>("Invalid comparison operator")),
        }
    }

    #[staticmethod]
    fn from_string(uuid: &str) -> PyResult<Uuid> {
        Ok(Uuid::from(uuid.to_string()))
    }

    fn to_string(&self) -> PyResult<String> {
        Ok(self.0.to_string())
    }
}

impl From<String> for Uuid {
    fn from(uuid: String) -> Self {
        Uuid(uuid::Uuid::parse_str(&uuid).unwrap())
    }
}

impl From<uuid::Uuid> for Uuid {
    fn from(uuid: uuid::Uuid) -> Self {
        Uuid(uuid)
    }
}

#[pyclass(module = "openportal")]
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Status(job::Status);

#[pymethods]
impl Status {
    #[new]
    fn new(status: &str) -> PyResult<Self> {
        match status.parse::<job::Status>() {
            Ok(status) => Ok(Self(status)),
            Err(e) => Err(PyErr::new::<PyOSError, _>(format!("{:?}", e))),
        }
    }

    fn __str__(&self) -> PyResult<String> {
        Ok(self.0.to_string())
    }

    fn __repr__(&self) -> PyResult<String> {
        self.__str__()
    }

    fn __copy__(&self) -> PyResult<Status> {
        Ok(self.clone())
    }

    fn __deepcopy__(&self, _memo: Py<PyAny>) -> PyResult<Status> {
        Ok(self.clone())
    }

    fn __richcmp__(&self, other: &Status, op: CompareOp) -> PyResult<bool> {
        match op {
            CompareOp::Eq => Ok(self.0 == other.0),
            CompareOp::Ne => Ok(self.0 != other.0),
            _ => Err(PyErr::new::<PyOSError, _>("Invalid comparison operator")),
        }
    }

    #[staticmethod]
    fn created() -> PyResult<Status> {
        Ok(Status(job::Status::Created))
    }

    #[staticmethod]
    fn pending() -> PyResult<Status> {
        Ok(Status(job::Status::Pending))
    }

    #[staticmethod]
    fn running() -> PyResult<Status> {
        Ok(Status(job::Status::Running))
    }

    #[staticmethod]
    fn complete() -> PyResult<Status> {
        Ok(Status(job::Status::Complete))
    }

    #[staticmethod]
    fn error() -> PyResult<Status> {
        Ok(Status(job::Status::Error))
    }

    #[staticmethod]
    fn duplicate() -> PyResult<Status> {
        Ok(Status(job::Status::Duplicate))
    }
}

impl From<job::Status> for Status {
    fn from(status: job::Status) -> Self {
        Status(status)
    }
}

#[pyclass(module = "openportal")]
#[derive(Debug, Clone, Serialize, Deserialize)]
struct UserIdentifier(grammar::UserIdentifier);

#[pymethods]
impl UserIdentifier {
    #[new]
    fn new(identifier: &str) -> PyResult<Self> {
        match grammar::UserIdentifier::parse(identifier) {
            Ok(user_identifier) => Ok(Self(user_identifier)),
            Err(e) => Err(PyErr::new::<PyOSError, _>(format!("{:?}", e))),
        }
    }

    fn __copy__(&self) -> PyResult<UserIdentifier> {
        Ok(self.clone())
    }

    fn __deepcopy__(&self, _memo: Py<PyAny>) -> PyResult<UserIdentifier> {
        Ok(self.clone())
    }

    fn __richcmp__(&self, other: &UserIdentifier, op: CompareOp) -> PyResult<bool> {
        match op {
            CompareOp::Eq => Ok(self.0 == other.0),
            CompareOp::Ne => Ok(self.0 != other.0),
            _ => Err(PyErr::new::<PyOSError, _>("Invalid comparison operator")),
        }
    }

    #[getter]
    fn username(&self) -> PyResult<String> {
        Ok(self.0.username().clone())
    }

    #[getter]
    fn project(&self) -> PyResult<String> {
        Ok(self.0.project().clone())
    }

    #[getter]
    fn portal(&self) -> PyResult<String> {
        Ok(self.0.portal().clone())
    }

    #[getter]
    fn project_identifier(&self) -> PyResult<ProjectIdentifier> {
        Ok(self.0.project_identifier().clone().into())
    }

    #[getter]
    fn portal_identifier(&self) -> PyResult<PortalIdentifier> {
        Ok(self.0.portal_identifier().clone().into())
    }

    fn __str__(&self) -> PyResult<String> {
        Ok(self.0.to_string())
    }

    fn __repr__(&self) -> PyResult<String> {
        self.__str__()
    }
}

impl From<grammar::UserIdentifier> for UserIdentifier {
    fn from(user_identifier: grammar::UserIdentifier) -> Self {
        UserIdentifier(user_identifier)
    }
}

#[pyclass(module = "openportal")]
#[derive(Debug, Clone, Serialize, Deserialize)]
struct ProjectIdentifier(grammar::ProjectIdentifier);

#[pymethods]
impl ProjectIdentifier {
    #[new]
    fn new(identifier: &str) -> PyResult<Self> {
        match grammar::ProjectIdentifier::parse(identifier) {
            Ok(project_identifier) => Ok(Self(project_identifier)),
            Err(e) => Err(PyErr::new::<PyOSError, _>(format!("{:?}", e))),
        }
    }

    #[getter]
    fn project(&self) -> PyResult<String> {
        Ok(self.0.project().clone())
    }

    #[getter]
    fn portal(&self) -> PyResult<String> {
        Ok(self.0.portal().clone())
    }

    #[getter]
    fn portal_identifier(&self) -> PyResult<PortalIdentifier> {
        Ok(self.0.portal_identifier().clone().into())
    }

    fn __str__(&self) -> PyResult<String> {
        Ok(self.0.to_string())
    }

    fn __repr__(&self) -> PyResult<String> {
        self.__str__()
    }

    fn __copy__(&self) -> PyResult<ProjectIdentifier> {
        Ok(self.clone())
    }

    fn __deepcopy__(&self, _memo: Py<PyAny>) -> PyResult<ProjectIdentifier> {
        Ok(self.clone())
    }

    fn __richcmp__(&self, other: &ProjectIdentifier, op: CompareOp) -> PyResult<bool> {
        match op {
            CompareOp::Eq => Ok(self.0 == other.0),
            CompareOp::Ne => Ok(self.0 != other.0),
            _ => Err(PyErr::new::<PyOSError, _>("Invalid comparison operator")),
        }
    }
}

impl From<grammar::ProjectIdentifier> for ProjectIdentifier {
    fn from(project_identifier: grammar::ProjectIdentifier) -> Self {
        ProjectIdentifier(project_identifier)
    }
}

#[pyclass(module = "openportal")]
#[derive(Debug, Clone, Serialize, Deserialize)]
struct PortalIdentifier(grammar::PortalIdentifier);

#[pymethods]
impl PortalIdentifier {
    #[new]
    fn new(identifier: &str) -> PyResult<Self> {
        match grammar::PortalIdentifier::parse(identifier) {
            Ok(portal_identifier) => Ok(Self(portal_identifier)),
            Err(e) => Err(PyErr::new::<PyOSError, _>(format!("{:?}", e))),
        }
    }

    #[getter]
    fn portal(&self) -> PyResult<String> {
        Ok(self.0.portal().clone())
    }

    fn __str__(&self) -> PyResult<String> {
        Ok(self.0.to_string())
    }

    fn __repr__(&self) -> PyResult<String> {
        self.__str__()
    }

    fn __copy__(&self) -> PyResult<PortalIdentifier> {
        Ok(self.clone())
    }

    fn __deepcopy__(&self, _memo: Py<PyAny>) -> PyResult<PortalIdentifier> {
        Ok(self.clone())
    }

    fn __richcmp__(&self, other: &PortalIdentifier, op: CompareOp) -> PyResult<bool> {
        match op {
            CompareOp::Eq => Ok(self.0 == other.0),
            CompareOp::Ne => Ok(self.0 != other.0),
            _ => Err(PyErr::new::<PyOSError, _>("Invalid comparison operator")),
        }
    }
}

impl From<grammar::PortalIdentifier> for PortalIdentifier {
    fn from(portal_identifier: grammar::PortalIdentifier) -> Self {
        PortalIdentifier(portal_identifier)
    }
}

#[pyclass(module = "openportal")]
#[derive(Debug, Clone, Serialize, Deserialize)]
struct UserMapping(grammar::UserMapping);

#[pymethods]
impl UserMapping {
    #[new]
    fn new(identifier: &str) -> PyResult<Self> {
        match grammar::UserMapping::parse(identifier) {
            Ok(user_mapping) => Ok(Self(user_mapping)),
            Err(e) => Err(PyErr::new::<PyOSError, _>(format!("{:?}", e))),
        }
    }

    #[getter]
    fn user(&self) -> PyResult<UserIdentifier> {
        Ok(self.0.user().clone().into())
    }

    #[getter]
    fn local_user(&self) -> PyResult<String> {
        Ok(self.0.local_user().to_string())
    }

    #[getter]
    fn local_group(&self) -> PyResult<String> {
        Ok(self.0.local_group().to_string())
    }

    fn __str__(&self) -> PyResult<String> {
        Ok(self.0.to_string())
    }

    fn __repr__(&self) -> PyResult<String> {
        self.__str__()
    }

    fn __copy__(&self) -> PyResult<UserMapping> {
        Ok(self.clone())
    }

    fn __deepcopy__(&self, _memo: Py<PyAny>) -> PyResult<UserMapping> {
        Ok(self.clone())
    }

    fn __richcmp__(&self, other: &UserMapping, op: CompareOp) -> PyResult<bool> {
        match op {
            CompareOp::Eq => Ok(self.0 == other.0),
            CompareOp::Ne => Ok(self.0 != other.0),
            _ => Err(PyErr::new::<PyOSError, _>("Invalid comparison operator")),
        }
    }
}

impl From<grammar::UserMapping> for UserMapping {
    fn from(user_mapping: grammar::UserMapping) -> Self {
        UserMapping(user_mapping)
    }
}

#[pyclass(module = "openportal")]
#[derive(Debug, Clone, Serialize, Deserialize)]
struct ProjectMapping(grammar::ProjectMapping);

#[pymethods]
impl ProjectMapping {
    #[new]
    fn new(identifier: &str) -> PyResult<Self> {
        match grammar::ProjectMapping::parse(identifier) {
            Ok(project_mapping) => Ok(Self(project_mapping)),
            Err(e) => Err(PyErr::new::<PyOSError, _>(format!("{:?}", e))),
        }
    }

    #[getter]
    fn project(&self) -> PyResult<ProjectIdentifier> {
        Ok(self.0.project().clone().into())
    }

    #[getter]
    fn local_group(&self) -> PyResult<String> {
        Ok(self.0.local_group().to_string())
    }

    fn __str__(&self) -> PyResult<String> {
        Ok(self.0.to_string())
    }

    fn __repr__(&self) -> PyResult<String> {
        self.__str__()
    }

    fn __copy__(&self) -> PyResult<ProjectMapping> {
        Ok(self.clone())
    }

    fn __deepcopy__(&self, _memo: Py<PyAny>) -> PyResult<ProjectMapping> {
        Ok(self.clone())
    }

    fn __richcmp__(&self, other: &ProjectMapping, op: CompareOp) -> PyResult<bool> {
        match op {
            CompareOp::Eq => Ok(self.0 == other.0),
            CompareOp::Ne => Ok(self.0 != other.0),
            _ => Err(PyErr::new::<PyOSError, _>("Invalid comparison operator")),
        }
    }
}

impl From<grammar::ProjectMapping> for ProjectMapping {
    fn from(project_mapping: grammar::ProjectMapping) -> Self {
        ProjectMapping(project_mapping)
    }
}

#[pyclass(module = "openportal")]
#[derive(Debug, Clone, Serialize, Deserialize)]
struct ProjectTemplate(grammar::ProjectTemplate);

#[pymethods]
impl ProjectTemplate {
    #[new]
    fn new(class: &str) -> PyResult<Self> {
        match grammar::ProjectTemplate::parse(class) {
            Ok(project_class) => Ok(Self(project_class)),
            Err(e) => Err(PyErr::new::<PyOSError, _>(format!("{:?}", e))),
        }
    }

    fn __str__(&self) -> PyResult<String> {
        Ok(self.0.to_string())
    }

    fn __repr__(&self) -> PyResult<String> {
        self.__str__()
    }

    fn __copy__(&self) -> PyResult<ProjectTemplate> {
        Ok(self.clone())
    }

    fn __deepcopy__(&self, _memo: Py<PyAny>) -> PyResult<ProjectTemplate> {
        Ok(self.clone())
    }

    fn __richcmp__(&self, other: &ProjectTemplate, op: CompareOp) -> PyResult<bool> {
        match op {
            CompareOp::Eq => Ok(self.0 == other.0),
            CompareOp::Ne => Ok(self.0 != other.0),
            _ => Err(PyErr::new::<PyOSError, _>("Invalid comparison operator")),
        }
    }
}

impl From<grammar::ProjectTemplate> for ProjectTemplate {
    fn from(project_class: grammar::ProjectTemplate) -> Self {
        ProjectTemplate(project_class)
    }
}

#[pyclass(module = "openportal")]
#[derive(Debug, Clone, Serialize, Deserialize)]
struct ProjectDetails(grammar::ProjectDetails);

#[pymethods]
impl ProjectDetails {
    #[new]
    fn new(details: &str) -> PyResult<Self> {
        match grammar::ProjectDetails::parse(details) {
            Ok(project_details) => Ok(Self(project_details)),
            Err(e) => Err(PyErr::new::<PyOSError, _>(format!("{:?}", e))),
        }
    }

    fn __str__(&self) -> PyResult<String> {
        Ok(self.0.to_string())
    }

    fn __repr__(&self) -> PyResult<String> {
        self.__str__()
    }

    fn __copy__(&self) -> PyResult<ProjectDetails> {
        Ok(self.clone())
    }

    fn __deepcopy__(&self, _memo: Py<PyAny>) -> PyResult<ProjectDetails> {
        Ok(self.clone())
    }

    fn __richcmp__(&self, other: &ProjectDetails, op: CompareOp) -> PyResult<bool> {
        match op {
            CompareOp::Eq => Ok(self.0 == other.0),
            CompareOp::Ne => Ok(self.0 != other.0),
            _ => Err(PyErr::new::<PyOSError, _>("Invalid comparison operator")),
        }
    }

    #[getter]
    fn name(&self) -> PyResult<Option<String>> {
        Ok(self.0.name())
    }

    #[setter]
    fn set_name(&mut self, name: &str) -> PyResult<()> {
        self.0.set_name(name);
        Ok(())
    }

    fn clear_name(&mut self) -> PyResult<()> {
        self.0.clear_name();
        Ok(())
    }

    #[getter]
    fn project_template(&self) -> PyResult<Option<ProjectTemplate>> {
        Ok(self.0.template().map(|pc| pc.into()))
    }

    #[setter]
    fn set_project_template(&mut self, template: &ProjectTemplate) -> PyResult<()> {
        self.0.set_template(template.0.clone());
        Ok(())
    }

    fn clear_project_template(&mut self) -> PyResult<()> {
        self.0.clear_template();
        Ok(())
    }

    #[getter]
    fn key(&self) -> PyResult<Option<String>> {
        Ok(self.0.key())
    }

    #[setter]
    fn set_key(&mut self, key: &str) -> PyResult<()> {
        self.0.set_key(key);
        Ok(())
    }

    fn clear_key(&mut self) -> PyResult<()> {
        self.0.clear_key();
        Ok(())
    }

    #[getter]
    fn description(&self) -> PyResult<Option<String>> {
        Ok(self.0.description())
    }

    #[setter]
    fn set_description(&mut self, description: &str) -> PyResult<()> {
        self.0.set_description(description);
        Ok(())
    }

    fn clear_description(&mut self) -> PyResult<()> {
        self.0.clear_description();
        Ok(())
    }

    #[getter]
    fn members(&self) -> PyResult<Option<HashMap<String, String>>> {
        Ok(self.0.members())
    }

    #[setter]
    fn set_members(&mut self, members: HashMap<String, String>) -> PyResult<()> {
        self.0.set_members(members);
        Ok(())
    }

    fn clear_members(&mut self) -> PyResult<()> {
        self.0.clear_members();
        Ok(())
    }

    fn add_member(&mut self, username: &str, role: &str) -> PyResult<()> {
        self.0.add_member(username, role);
        Ok(())
    }

    fn remove_member(&mut self, username: &str) -> PyResult<()> {
        self.0.remove_member(username);
        Ok(())
    }

    #[getter]
    fn start_date(&self) -> PyResult<Option<chrono::NaiveDate>> {
        Ok(self.0.start_date().map(|date| date.to_chrono()))
    }

    #[setter]
    fn set_start_date(&mut self, start_date: Option<chrono::NaiveDate>) -> PyResult<()> {
        if let Some(date) = start_date {
            self.0.set_start_date(grammar::Date::from_chrono(&date));
        } else {
            self.0.clear_start_date();
        }
        Ok(())
    }

    fn clear_start_date(&mut self) -> PyResult<()> {
        self.0.clear_start_date();
        Ok(())
    }

    #[getter]
    fn end_date(&self) -> PyResult<Option<chrono::NaiveDate>> {
        Ok(self.0.end_date().map(|date| date.to_chrono()))
    }

    #[setter]
    fn set_end_date(&mut self, end_date: Option<chrono::NaiveDate>) -> PyResult<()> {
        if let Some(date) = end_date {
            self.0.set_end_date(grammar::Date::from_chrono(&date));
        } else {
            self.0.clear_end_date();
        }
        Ok(())
    }

    fn clear_end_date(&mut self) -> PyResult<()> {
        self.0.clear_end_date();
        Ok(())
    }

    #[getter]
    fn allocation(&self) -> PyResult<Option<Allocation>> {
        Ok(self.0.allocation().map(|allocation| allocation.into()))
    }

    #[setter]
    fn set_allocation(&mut self, allocation: Option<Allocation>) -> PyResult<()> {
        if let Some(allocation) = allocation {
            self.0.set_allocation(allocation.0);
        } else {
            self.0.clear_allocation();
        }
        Ok(())
    }

    fn clear_allocation(&mut self) -> PyResult<()> {
        self.0.clear_allocation();
        Ok(())
    }

    fn merge(&self, other: &ProjectDetails) -> PyResult<ProjectDetails> {
        match self.0.merge(&other.0) {
            Ok(merged) => Ok(merged.into()),
            Err(e) => Err(PyErr::new::<PyOSError, _>(format!("{:?}", e))),
        }
    }
}

impl From<grammar::ProjectDetails> for ProjectDetails {
    fn from(project_details: grammar::ProjectDetails) -> Self {
        ProjectDetails(project_details)
    }
}

///
/// Run the passed command on the OpenPortal system.
/// This will return a Job object that can be used to query the
/// status of the job and get the results.
///
/// By default, this will not wait for the job to finish. If you
/// want to wait for the job to finish, pass a maximum number of
/// milliseconds to wait as 'max_ms', or a negative number if you want
/// to wait indefinitely.
///
#[pyfunction]
#[pyo3(signature = (command, max_ms=0))]
fn run(command: String, max_ms: i64) -> PyResult<Job> {
    let mut job: Job = match call_post::<job::Job>("run", serde_json::json!({"command": command})) {
        Ok(response) => response.into(),
        Err(e) => return Err(PyErr::new::<PyOSError, _>(format!("{:?}", e))),
    };

    job.update()?;

    if max_ms != 0 {
        match job.wait(max_ms) {
            Ok(_) => Ok(job),
            Err(e) => Err(PyErr::new::<PyOSError, _>(format!("{:?}", e))),
        }
    } else {
        Ok(job)
    }
}

///
/// Get the status of the passed job on the OpenPortal System
/// This will return the job updated to the latest version.
///
#[pyfunction]
fn status(job: Job) -> PyResult<Job> {
    match call_post::<job::Job>("status", serde_json::json!({"job": job.0.id().to_string()})) {
        Ok(response) => Ok(response.into()),
        Err(e) => Err(PyErr::new::<PyOSError, _>(format!("{:?}", e))),
    }
}

///
/// Return the Job with the specified ID. Raises an error if the
/// job does not exist.
///
#[pyfunction]
fn get(py: Python<'_>, job_id: Py<PyAny>) -> PyResult<Job> {
    let job_id = match job_id.extract::<Uuid>(py) {
        Ok(uid) => uid.to_string()?,
        Err(_) => match job_id.extract::<String>(py) {
            Ok(uid) => uid,
            Err(_) => {
                return Err(PyErr::new::<PyOSError, _>(
                    "Job ID must be a string or a Uuid",
                ))
            }
        },
    };

    match call_post::<job::Job>("status", serde_json::json!({"job": job_id})) {
        Ok(response) => Ok(response.into()),
        Err(e) => Err(PyErr::new::<PyOSError, _>(format!("{:?}", e))),
    }
}

///
/// Fetch all of the jobs that OpenPortal has passed back to us
/// to run
///
#[pyfunction]
fn fetch_jobs() -> PyResult<Vec<Job>> {
    match call_get::<Vec<job::Job>>("fetch_jobs") {
        Ok(response) => Ok(response.into_iter().map(|j| j.into()).collect()),
        Err(e) => Err(PyErr::new::<PyOSError, _>(format!("{:?}", e))),
    }
}

#[pyfunction]
fn fetch_job(py: Python<'_>, job_id: Py<PyAny>) -> PyResult<Job> {
    let uid: uuid::Uuid = match job_id.extract::<Uuid>(py) {
        Ok(uid) => uid.0,
        Err(_) => match job_id.extract::<Job>(py) {
            Ok(job) => job.0.id(),
            Err(_) => match job_id.extract::<String>(py) {
                Ok(uid) => uuid::Uuid::parse_str(&uid)
                    .map_err(|_| PyErr::new::<PyOSError, _>("Job ID must be a string or a Uuid"))?,
                Err(_) => {
                    return Err(PyErr::new::<PyOSError, _>(
                        "Job ID must be a string or a Uuid",
                    ))
                }
            },
        },
    };

    match call_post::<job::Job>("fetch_job", serde_json::json!(uid)) {
        Ok(response) => Ok(response.into()),
        Err(e) => Err(PyErr::new::<PyOSError, _>(format!("{:?}", e))),
    }
}

#[pyfunction]
fn add_offerings(offerings: Vec<Destination>) -> PyResult<Vec<Destination>> {
    let offerings: Vec<destination::Destination> = offerings.into_iter().map(|d| d.0).collect();

    match call_post::<destination::Destinations>(
        "add_offerings",
        serde_json::json!(destination::Destinations::new(&offerings)),
    ) {
        Ok(offerings) => Ok(offerings.iter().map(|d| d.clone().into()).collect()),
        Err(e) => Err(PyErr::new::<PyOSError, _>(format!("{:?}", e))),
    }
}

#[pyfunction]
fn remove_offerings(offerings: Vec<Destination>) -> PyResult<Vec<Destination>> {
    let offerings: Vec<destination::Destination> = offerings.into_iter().map(|d| d.0).collect();

    match call_post::<destination::Destinations>(
        "remove_offerings",
        serde_json::json!(destination::Destinations::new(&offerings)),
    ) {
        Ok(offerings) => Ok(offerings.iter().map(|d| d.clone().into()).collect()),
        Err(e) => Err(PyErr::new::<PyOSError, _>(format!("{:?}", e))),
    }
}

#[pyfunction]
fn get_offerings() -> PyResult<Vec<Destination>> {
    match call_get::<Vec<destination::Destination>>("get_offerings") {
        Ok(offerings) => Ok(offerings.iter().map(|d| d.clone().into()).collect()),
        Err(e) => Err(PyErr::new::<PyOSError, _>(format!("{:?}", e))),
    }
}

#[pyfunction]
fn sync_offerings(offerings: Vec<Destination>) -> PyResult<Vec<Destination>> {
    let offerings: Vec<destination::Destination> = offerings.into_iter().map(|d| d.0).collect();

    match call_post::<destination::Destinations>(
        "sync_offerings",
        serde_json::json!(destination::Destinations::new(&offerings)),
    ) {
        Ok(offerings) => Ok(offerings.iter().map(|d| d.clone().into()).collect()),
        Err(e) => Err(PyErr::new::<PyOSError, _>(format!("{:?}", e))),
    }
}

#[pyfunction]
fn get_portal() -> PyResult<PortalIdentifier> {
    match call_get::<grammar::PortalIdentifier>("get_portal") {
        Ok(portal) => Ok(portal.into()),
        Err(e) => Err(PyErr::new::<PyOSError, _>(format!("{:?}", e))),
    }
}

///
/// Send back the result of us running a job that was passed to us by
/// OpenPortal.
///
#[pyfunction]
fn send_result(job: Job) -> PyResult<()> {
    match call_post::<Health>("send_result", serde_json::json!(job.0)) {
        Ok(_) => Ok(()),
        Err(e) => Err(PyErr::new::<PyOSError, _>(format!("{:?}", e))),
    }
}

#[pymodule]
fn openportal(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(add_offerings, m)?)?;
    m.add_function(wrap_pyfunction!(load_config, m)?)?;
    m.add_function(wrap_pyfunction!(fetch_job, m)?)?;
    m.add_function(wrap_pyfunction!(fetch_jobs, m)?)?;
    m.add_function(wrap_pyfunction!(get, m)?)?;
    m.add_function(wrap_pyfunction!(get_offerings, m)?)?;
    m.add_function(wrap_pyfunction!(get_portal, m)?)?;
    m.add_function(wrap_pyfunction!(diagnostics, m)?)?;
    m.add_function(wrap_pyfunction!(health, m)?)?;
    m.add_function(wrap_pyfunction!(is_config_loaded, m)?)?;
    m.add_function(wrap_pyfunction!(initialize_tracing, m)?)?;
    m.add_function(wrap_pyfunction!(remove_offerings, m)?)?;
    m.add_function(wrap_pyfunction!(restart, m)?)?;
    m.add_function(wrap_pyfunction!(run, m)?)?;
    m.add_function(wrap_pyfunction!(send_result, m)?)?;
    m.add_function(wrap_pyfunction!(status, m)?)?;
    m.add_function(wrap_pyfunction!(sync_offerings, m)?)?;

    m.add_class::<Health>()?;
    m.add_class::<RestartResponse>()?;
    m.add_class::<Diagnostics>()?;
    m.add_class::<DiagnosticsReport>()?;
    m.add_class::<FailedJobEntry>()?;
    m.add_class::<SlowJobEntry>()?;
    m.add_class::<ExpiredJobEntry>()?;
    m.add_class::<RunningJobEntry>()?;
    m.add_class::<Job>()?;
    m.add_class::<UserIdentifier>()?;
    m.add_class::<ProjectIdentifier>()?;
    m.add_class::<PortalIdentifier>()?;
    m.add_class::<UserMapping>()?;
    m.add_class::<ProjectMapping>()?;
    m.add_class::<Uuid>()?;
    m.add_class::<Destination>()?;
    m.add_class::<Instruction>()?;
    m.add_class::<Status>()?;
    m.add_class::<DateRange>()?;
    m.add_class::<Node>()?;
    m.add_class::<Allocation>()?;
    m.add_class::<Usage>()?;
    m.add_class::<UsageReport>()?;
    m.add_class::<ProjectUsageReport>()?;
    m.add_class::<DailyProjectUsageReport>()?;
    m.add_class::<ProjectDetails>()?;
    m.add_class::<ProjectTemplate>()?;

    Ok(())
}
