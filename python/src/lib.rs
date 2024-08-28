use anyhow::{Context, Result};
use pyo3::exceptions::PyOSError;
use pyo3::prelude::*;
use serde::{de::DeserializeOwned, Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthResponse {
    pub status: String,
}

fn call_get<T>(api_url: &url::Url, token: &String) -> Result<T>
where
    T: DeserializeOwned,
{
    let result = reqwest::blocking::Client::new()
        .get(format!("{api_url}health"))
        .query(&[("openportal-version", "0.01")])
        .header("Accept", "application/json")
        .header("Authorization", format!("Token {token}"))
        .send()
        .context("Could not call function.")?;

    tracing::info!("Response: {:?}", result);

    if result.status().is_success() {
        Ok(result.json::<T>().context("Could not decode from json")?)
    } else {
        Err(anyhow::anyhow!("Could not get response text."))
    }
}

/// Formats the sum of two numbers as string.
#[pyfunction]
fn health() -> PyResult<String> {
    let token = "1234567890".to_string();
    let url = url::Url::parse("http://localhost:3000/").unwrap();

    match call_get::<HealthResponse>(&url, &token) {
        Ok(response) => Ok(response.status),
        Err(e) => Err(PyErr::new::<PyOSError, _>(format!("{:?}", e))),
    }
}

/// A Python module implemented in Rust. The name of this function must match
/// the `lib.name` setting in the `Cargo.toml`, else Python will not be able to
/// import the module.
#[pymodule]
fn openportal(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(health, m)?)?;
    Ok(())
}
