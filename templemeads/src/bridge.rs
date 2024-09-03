// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use anyhow::Result;
use chrono::{DateTime, Utc};
use paddington::SecretKey;
use secrecy::ExposeSecret;

///
/// Return the OpenPortal authorisation header for the passed datetime,
/// protocol, function and (optional) arguments, signed with the passed
/// key.
///
pub fn sign_api_call(
    key: &SecretKey,
    date: &DateTime<Utc>,
    protocol: &str,
    function: &str,
    arguments: &Option<serde_json::Value>,
) -> Result<String, anyhow::Error> {
    let date = date.format("%a, %d %b %Y %H:%M:%S GMT").to_string();

    let call_string = match arguments {
        Some(args) => format!(
            "{}\napplication/json\n{}\n{}\n{}",
            protocol, date, function, args
        ),
        None => format!("{}\napplication/json\n{}\n{}", protocol, date, function),
    };

    tracing::info!("Signing: {}", call_string);

    let signature = key.expose_secret().sign(call_string)?;
    Ok(format!("OpenPortal {}", signature))
}
