use pyo3::prelude::*;

/// Formats the sum of two numbers as string.
#[pyfunction]
fn call_temple_meads() -> PyResult<String> {
    Ok(templemeads::agent::test_function())
}

/// A Python module implemented in Rust. The name of this function must match
/// the `lib.name` setting in the `Cargo.toml`, else Python will not be able to
/// import the module.
#[pymodule]
fn openportal(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(call_temple_meads, m)?)?;
    Ok(())
}
