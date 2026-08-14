// SPDX-FileCopyrightText: © 2026 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use crate::error::Error;

use std::any::Any;
use std::sync::OnceLock;

///
/// Rust statics cannot themselves be generic, but templemeads needs a
/// handful of process-global singletons (the per-peer job boards, the
/// bridge board, the notification queues) whose types are generic over
/// `L: Domain`. Since a compiled binary only ever chooses one concrete
/// `L`, this stores the singleton type-erased behind a single
/// `OnceLock<Box<dyn Any>>` and downcasts back on every access - the
/// downcast can only fail if a single process somehow mixed two
/// different `Domain`s against the same static, which never happens in
/// practice, but is reported as a real `Error` rather than unwrapped.
///
pub(crate) fn get_or_init<T>(
    once: &'static OnceLock<Box<dyn Any + Send + Sync>>,
    init: impl FnOnce() -> T,
) -> Result<&'static T, Error>
where
    T: Send + Sync + 'static,
{
    once.get_or_init(|| Box::new(init()))
        .downcast_ref::<T>()
        .ok_or_else(|| {
            Error::Unknown(
                "Internal error: mismatched Domain type in process-global state".to_string(),
            )
        })
}
