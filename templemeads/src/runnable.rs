// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use crate::error::Error;
use crate::job::{Envelope, Job};

use anyhow::Result;
use std::future::Future;
use std::pin::Pin;

#[macro_export]
macro_rules! async_runnable {(
    $( #[$attr:meta] )* // includes doc strings
    $pub:vis
    async
    fn $fname:ident ( $($args:tt)* ) $(-> $Ret:ty)?
    {
        $($body:tt)*
    }
) => (
    $( #[$attr] )*
    #[allow(unused_parens)]
    $pub
    fn $fname ( $($args)* ) -> ::std::pin::Pin<::std::boxed::Box<
        dyn Send + ::std::future::Future<Output = ($($Ret)?)>
    >>
    {
        Box::pin(async move { $($body)* })
    }
)}

pub type AsyncRunnable = fn(
    Envelope,
) -> Pin<
    Box<
        dyn Future<Output = Result<Job, Error>> // future API / pollable
            + Send, // required by non-single-threaded executors
    >,
>;

async_runnable! {
    pub async fn default_runner(envelope: Envelope) -> Result<Job, Error>
    {
        tracing::debug!("Using the default runner for job from {} to {}", envelope.sender(), envelope.recipient());
        let result = envelope.job().execute().await?;

        Ok(result)
    }
}
