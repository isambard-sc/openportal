// SPDX-FileCopyrightText: © 2026 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

//! Python conversions (`pyo3`), to and from the `datetime` module:
//!
//! | Rust | Python |
//! |---|---|
//! | `NaiveDate` | `datetime.date` |
//! | `NaiveDateTime` | a naive `datetime.datetime` (no `tzinfo`) |
//! | `DateTime<Utc>` | an aware `datetime.datetime` in `timezone.utc` |
//!
//! An aware `datetime` in any time zone converts to a `DateTime<Utc>` at the
//! same instant; a naive one is refused, since it does not say which instant
//! it is. Python has microseconds, so nanoseconds are truncated on the way
//! out. Every field is read by attribute, which works under the stable ABI.

use pyo3::exceptions::{PyTypeError, PyValueError};
use pyo3::types::{PyAnyMethods, PyDate, PyDateTime, PyTzInfo};
use pyo3::{Borrowed, Bound, FromPyObject, IntoPyObject, PyAny, PyErr, PyResult, Python};

use crate::{DateTime, Datelike, NaiveDate, NaiveDateTime, NaiveTime, Timelike, Utc};

fn to_u8(value: u32) -> u8 {
    u8::try_from(value).unwrap_or(u8::MAX)
}

fn read_date(ob: &Bound<'_, PyAny>) -> PyResult<NaiveDate> {
    NaiveDate::from_ymd_opt(
        ob.getattr("year")?.extract()?,
        ob.getattr("month")?.extract()?,
        ob.getattr("day")?.extract()?,
    )
    .ok_or_else(|| PyValueError::new_err("invalid or out-of-range date"))
}

fn read_time(ob: &Bound<'_, PyAny>) -> PyResult<NaiveTime> {
    NaiveTime::from_hms_micro_opt(
        ob.getattr("hour")?.extract()?,
        ob.getattr("minute")?.extract()?,
        ob.getattr("second")?.extract()?,
        ob.getattr("microsecond")?.extract()?,
    )
    .ok_or_else(|| PyValueError::new_err("invalid or out-of-range time"))
}

fn new_datetime<'py>(
    py: Python<'py>,
    naive: &NaiveDateTime,
    tzinfo: Option<&Bound<'py, PyTzInfo>>,
) -> PyResult<Bound<'py, PyDateTime>> {
    PyDateTime::new(
        py,
        naive.year(),
        to_u8(naive.month()),
        to_u8(naive.day()),
        to_u8(naive.hour()),
        to_u8(naive.minute()),
        to_u8(naive.second()),
        naive.nanosecond() / 1000,
        tzinfo,
    )
}

impl<'py> IntoPyObject<'py> for NaiveDate {
    type Target = PyDate;
    type Output = Bound<'py, PyDate>;
    type Error = PyErr;

    fn into_pyobject(self, py: Python<'py>) -> PyResult<Self::Output> {
        PyDate::new(py, self.year(), to_u8(self.month()), to_u8(self.day()))
    }
}

impl<'py> IntoPyObject<'py> for &NaiveDate {
    type Target = PyDate;
    type Output = Bound<'py, PyDate>;
    type Error = PyErr;

    fn into_pyobject(self, py: Python<'py>) -> PyResult<Self::Output> {
        (*self).into_pyobject(py)
    }
}

impl FromPyObject<'_, '_> for NaiveDate {
    type Error = PyErr;

    fn extract(ob: Borrowed<'_, '_, PyAny>) -> PyResult<Self> {
        let date = ob.cast::<PyDate>()?;
        read_date(date.as_any())
    }
}

impl<'py> IntoPyObject<'py> for NaiveDateTime {
    type Target = PyDateTime;
    type Output = Bound<'py, PyDateTime>;
    type Error = PyErr;

    fn into_pyobject(self, py: Python<'py>) -> PyResult<Self::Output> {
        new_datetime(py, &self, None)
    }
}

impl<'py> IntoPyObject<'py> for &NaiveDateTime {
    type Target = PyDateTime;
    type Output = Bound<'py, PyDateTime>;
    type Error = PyErr;

    fn into_pyobject(self, py: Python<'py>) -> PyResult<Self::Output> {
        (*self).into_pyobject(py)
    }
}

impl FromPyObject<'_, '_> for NaiveDateTime {
    type Error = PyErr;

    fn extract(ob: Borrowed<'_, '_, PyAny>) -> PyResult<Self> {
        let datetime = ob.cast::<PyDateTime>()?;

        if !datetime.getattr("tzinfo")?.is_none() {
            return Err(PyTypeError::new_err("expected a datetime without tzinfo"));
        }

        Ok(NaiveDateTime::new(
            read_date(datetime.as_any())?,
            read_time(datetime.as_any())?,
        ))
    }
}

impl<'py> IntoPyObject<'py> for DateTime<Utc> {
    type Target = PyDateTime;
    type Output = Bound<'py, PyDateTime>;
    type Error = PyErr;

    fn into_pyobject(self, py: Python<'py>) -> PyResult<Self::Output> {
        let utc = PyTzInfo::utc(py)?.to_owned();
        new_datetime(py, &self.naive_utc(), Some(&utc))
    }
}

impl<'py> IntoPyObject<'py> for &DateTime<Utc> {
    type Target = PyDateTime;
    type Output = Bound<'py, PyDateTime>;
    type Error = PyErr;

    fn into_pyobject(self, py: Python<'py>) -> PyResult<Self::Output> {
        (*self).into_pyobject(py)
    }
}

impl FromPyObject<'_, '_> for DateTime<Utc> {
    type Error = PyErr;

    fn extract(ob: Borrowed<'_, '_, PyAny>) -> PyResult<Self> {
        let datetime = ob.cast::<PyDateTime>()?;

        if datetime.getattr("tzinfo")?.is_none() {
            return Err(PyTypeError::new_err(
                "expected a datetime with non-None tzinfo",
            ));
        }

        let utc = datetime.call_method1("astimezone", (PyTzInfo::utc(ob.py())?,))?;

        Ok(NaiveDateTime::new(read_date(&utc)?, read_time(&utc)?).and_utc())
    }
}

/// Type information for `.pyi` stubs.
#[cfg(feature = "stub-gen")]
mod stubs {
    use pyo3::types::PyAnyMethods;
    use pyo3::{Bound, PyAny, PyResult, Python};
    use pyo3_stub_gen::runtime::PyRuntimeType;
    use pyo3_stub_gen::{PyStubType, TypeInfo};

    use crate::{DateTime, NaiveDate, NaiveDateTime, Utc};

    macro_rules! stub {
        ($ty:ty, $name:literal) => {
            impl PyStubType for $ty {
                fn type_output() -> TypeInfo {
                    TypeInfo::with_module(concat!("datetime.", $name), "datetime".into())
                }
            }

            impl PyRuntimeType for $ty {
                fn runtime_type_object(py: Python<'_>) -> PyResult<Bound<'_, PyAny>> {
                    py.import("datetime")?.getattr($name)
                }
            }
        };
    }

    stub!(NaiveDate, "date");
    stub!(NaiveDateTime, "datetime");
    stub!(DateTime<Utc>, "datetime");
}
