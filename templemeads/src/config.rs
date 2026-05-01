// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use tracing_subscriber::prelude::*;

use crate::diagnostics::RingBufferLayer;

pub fn initialise_tracing() {
    // make sure that we default to "INFO" if the RUST_LOG environment variable is not set
    match std::env::var("RUST_LOG") {
        Ok(_) => {}
        Err(_) => {
            std::env::set_var("RUST_LOG", "INFO");
        }
    }

    let base = tracing_subscriber::registry()
        .with(tracing_subscriber::EnvFilter::from_default_env())
        .with(RingBufferLayer);

    let format = std::env::var("RUST_LOG_FORMAT")
        .map(|s| s.to_lowercase())
        .unwrap_or_default();

    match format.as_str() {
        "json" => base.with(tracing_subscriber::fmt::layer().json()).init(),
        "pretty" => base.with(tracing_subscriber::fmt::layer().pretty()).init(),
        _ => base.with(tracing_subscriber::fmt::layer()).init(),
    }
}
