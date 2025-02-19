// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use anyhow::{Context, Result};
use chrono::Utc;
use once_cell::sync::Lazy;
use paddington::SecretKey;
use pyo3::basic::CompareOp;
use pyo3::exceptions::PyOSError;
use pyo3::prelude::*;
use pyo3::types::{PyDate, PyDateTime, PyList, PyNone, PyString};
use pyo3::{IntoPyObject, PyResult, Python};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::path;
use std::sync::RwLock;
use templemeads::destination;
use templemeads::grammar;
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
    templemeads::config::initialise_tracing();
    Ok(())
}

///
/// Return type for the health function
///
#[pyclass(module = "openportal")]
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

    fn __copy__(&self) -> PyResult<Health> {
        Ok(self.clone())
    }

    fn __deepcopy__(&self, _memo: Py<PyAny>) -> PyResult<Health> {
        Ok(self.clone())
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
    fn state(&self) -> PyResult<Status> {
        Ok(self.0.state().into())
    }

    #[getter]
    fn created<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyDateTime>> {
        PyDateTime::from_timestamp(
            py,
            self.0.created().timestamp() as f64,
            Some(&pyo3::types::timezone_utc(py)),
        )
    }

    #[getter]
    fn changed<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyDateTime>> {
        PyDateTime::from_timestamp(
            py,
            self.0.changed().timestamp() as f64,
            Some(&pyo3::types::timezone_utc(py)),
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
                    None => Ok(PyNone::get(py).as_ref().clone()),
                }
            }
            "bool" => {
                let result = match self.0.result::<bool>() {
                    Ok(result) => result,
                    Err(e) => return Err(PyErr::new::<PyOSError, _>(format!("{:?}", e))),
                };

                match result {
                    Some(result) => Ok((result as u64).into_pyobject(py)?.into_any()),
                    None => Ok(PyNone::get(py).as_ref().clone()),
                }
            }
            "None" => Ok(PyNone::get(py).as_ref().clone()),
            "UserIdentifier" => {
                let result = match self.0.result::<grammar::UserIdentifier>() {
                    Ok(result) => result,
                    Err(e) => return Err(PyErr::new::<PyOSError, _>(format!("{:?}", e))),
                };

                match result {
                    Some(result) => Ok(UserIdentifier::from(result).into_pyobject(py)?.into_any()),
                    None => Ok(PyNone::get(py).as_ref().clone()),
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
                    None => Ok(PyNone::get(py).as_ref().clone()),
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
                    None => Ok(PyNone::get(py).as_ref().clone()),
                }
            }
            "UserMapping" => {
                let result = match self.0.result::<grammar::UserMapping>() {
                    Ok(result) => result,
                    Err(e) => return Err(PyErr::new::<PyOSError, _>(format!("{:?}", e))),
                };

                match result {
                    Some(result) => Ok(UserMapping::from(result).into_pyobject(py)?.into_any()),
                    None => Ok(PyNone::get(py).as_ref().clone()),
                }
            }
            "ProjectMapping" => {
                let result = match self.0.result::<grammar::ProjectMapping>() {
                    Ok(result) => result,
                    Err(e) => return Err(PyErr::new::<PyOSError, _>(format!("{:?}", e))),
                };

                match result {
                    Some(result) => Ok(ProjectMapping::from(result).into_pyobject(py)?.into_any()),
                    None => Ok(PyNone::get(py).as_ref().clone()),
                }
            }
            "UsageReport" => {
                let result = match self.0.result::<usagereport::UsageReport>() {
                    Ok(result) => result,
                    Err(e) => return Err(PyErr::new::<PyOSError, _>(format!("{:?}", e))),
                };

                match result {
                    Some(result) => Ok(UsageReport::from(result).into_pyobject(py)?.into_any()),
                    None => Ok(PyNone::get(py).as_ref().clone()),
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
                    None => Ok(PyNone::get(py).as_ref().clone()),
                }
            }
            "Usage" => {
                let result = match self.0.result::<usagereport::Usage>() {
                    Ok(result) => result,
                    Err(e) => return Err(PyErr::new::<PyOSError, _>(format!("{:?}", e))),
                };

                match result {
                    Some(result) => Ok(Usage::from(result).into_pyobject(py)?.into_any()),
                    None => Ok(PyNone::get(py).as_ref().clone()),
                }
            }
            "DateRange" => {
                let result = match self.0.result::<grammar::DateRange>() {
                    Ok(result) => result,
                    Err(e) => return Err(PyErr::new::<PyOSError, _>(format!("{:?}", e))),
                };

                match result {
                    Some(result) => Ok(DateRange::from(result).into_pyobject(py)?.into_any()),
                    None => Ok(PyNone::get(py).as_ref().clone()),
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
                    None => Ok(PyNone::get(py).as_ref().clone()),
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
                    None => Ok(PyNone::get(py).as_ref().clone()),
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
                    None => Ok(PyNone::get(py).as_ref().clone()),
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
                    None => Ok(PyNone::get(py).as_ref().clone()),
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
                    None => Ok(PyNone::get(py).as_ref().clone()),
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
                    None => Ok(PyNone::get(py).as_ref().clone()),
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
}

impl From<usagereport::ProjectUsageReport> for ProjectUsageReport {
    fn from(project_usage_report: usagereport::ProjectUsageReport) -> Self {
        ProjectUsageReport(project_usage_report)
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

#[pymodule]
fn openportal(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(load_config, m)?)?;
    m.add_function(wrap_pyfunction!(is_config_loaded, m)?)?;
    m.add_function(wrap_pyfunction!(initialize_tracing, m)?)?;
    m.add_function(wrap_pyfunction!(health, m)?)?;
    m.add_function(wrap_pyfunction!(run, m)?)?;
    m.add_function(wrap_pyfunction!(status, m)?)?;
    m.add_function(wrap_pyfunction!(get, m)?)?;

    m.add_class::<Health>()?;
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
    m.add_class::<Usage>()?;
    m.add_class::<UsageReport>()?;
    m.add_class::<ProjectUsageReport>()?;

    Ok(())
}
