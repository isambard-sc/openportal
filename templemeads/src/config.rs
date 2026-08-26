// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use tracing_subscriber::prelude::*;

use crate::diagnostics::RingBufferLayer;

pub fn initialise_tracing() {
    let base = base_layers();

    match log_format().as_str() {
        "json" => base.with(tracing_subscriber::fmt::layer().json()).init(),
        "pretty" => base.with(tracing_subscriber::fmt::layer().pretty()).init(),
        _ => base.with(tracing_subscriber::fmt::layer()).init(),
    }
}

///
/// As `initialise_tracing`, but writing to standard error.
///
/// For a command-line tool whose *output* is what goes to standard output. A
/// tool that logs its progress to the same stream as its report cannot have
/// that report redirected to a file without the progress landing in it, which
/// is the difference between something an operator can pipe and something they
/// have to clean up by hand.
///
pub fn initialise_tracing_to_stderr() {
    let base = base_layers();

    match log_format().as_str() {
        "json" => base
            .with(
                tracing_subscriber::fmt::layer()
                    .json()
                    .with_writer(std::io::stderr),
            )
            .init(),
        "pretty" => base
            .with(
                tracing_subscriber::fmt::layer()
                    .pretty()
                    .with_writer(std::io::stderr),
            )
            .init(),
        _ => base
            .with(tracing_subscriber::fmt::layer().with_writer(std::io::stderr))
            .init(),
    }
}

fn base_layers() -> impl tracing_subscriber::layer::SubscriberExt
       + for<'a> tracing_subscriber::registry::LookupSpan<'a>
       + Send
       + Sync {
    // make sure that we default to "INFO" if the RUST_LOG environment variable is not set
    match std::env::var("RUST_LOG") {
        Ok(_) => {}
        Err(_) => {
            std::env::set_var("RUST_LOG", "INFO");
        }
    }

    tracing_subscriber::registry()
        .with(tracing_subscriber::EnvFilter::from_default_env())
        .with(RingBufferLayer)
}

fn log_format() -> String {
    std::env::var("RUST_LOG_FORMAT")
        .map(|s| s.to_lowercase())
        .unwrap_or_default()
}
