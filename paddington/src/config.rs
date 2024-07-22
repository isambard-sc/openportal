// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use std::io::Error as IOError;

#[derive(Debug)]
pub struct Config {
    pub server: String,
    pub port: u16,
}

pub fn load() -> Result<Config, IOError> {
    let config = Config {
        server: "localhost".to_string(),
        port: 8080,
    };
    Ok(config)
}
