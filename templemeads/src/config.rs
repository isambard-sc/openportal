// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

pub fn initialise_tracing() {
    let sub = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env());

    match std::env::var("RUST_LOG_FORMAT") {
        Ok(format) => {
            let format = format.to_lowercase();
            match format.as_str() {
                "json" => {
                    sub.json().init();
                }
                "pretty" => {
                    sub.pretty().init();
                }
                _ => {
                    sub.init();
                }
            }
        }
        Err(_) => sub.init(),
    };
}
