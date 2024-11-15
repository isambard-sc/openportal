// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use anyhow::Context;
use anyhow::Result;
use once_cell::sync::Lazy;
use reqwest::Client;
use secrecy::Secret;
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::collections::HashMap;
use templemeads::Error;
use tokio::sync::{Mutex, MutexGuard};

#[derive(Debug, Clone, Serialize, Deserialize)]
struct FreeResponse {
    result: serde_json::Value,
    principal: serde_json::Value,
    error: serde_json::Value,
    id: u16,
}

///
/// Call a post URL on the slurmrestd server described in 'auth'.
///
async fn call_post<T>(
    func: &str,
    args: Option<Vec<String>>,
    kwargs: Option<HashMap<String, String>>,
) -> Result<T, Error>
where
    T: DeserializeOwned,
{
    // get the auth details from the global Slurm client
    let mut auth = auth().await?;
    auth.num_reconnects = 0;

    let url = format!("{}/ipa/session/json", &auth.server);

    let mut kwargs = kwargs.unwrap_or_default();

    // the payload is a json object that contains the method, the parameters
    // (as an array, plus a dict of the version) and a random id. The id
    // will be passed back to us in the response.
    let payload = serde_json::json!({
        "method": func,
        "params": [args.clone().unwrap_or_default(), kwargs.clone()],
    });

    let client = Client::builder()
        .build()
        .context("Could not build client")?;

    let mut result = client
        .post(&url)
        .header("Referer", format!("{}/ipa", &auth.server))
        .header("Content-Type", "application/json")
        .header("Accept", "application/json")
        .json(&payload)
        .send()
        .await
        .with_context(|| format!("Could not call function: {}", payload))?;

    // if this is an authorisation error, try to reconnect
    while result.status().as_u16() == 401 {
        auth.num_reconnects += 1;

        if auth.num_reconnects > 3 {
            return Err(Error::Call(format!(
                "Authorisation (401) error: Could not get response for function: {}. Status: {}. Response: {:?}",
                payload,
                result.status(),
                    result
                )));
        }

        tracing::error!("Authorisation (401) error. Reconnecting.");

        match login(&auth.server, &auth.user, &auth.token_command).await {
            Ok(jwt) => {
                auth.jwt = jwt;

                // create a new client with the new cookies
                let client = Client::builder()
                    .build()
                    .context("Could not build client")?;

                // retry the call
                result = client
                    .post(&url)
                    .header("Referer", format!("{}/ipa", &auth.server))
                    .header("Content-Type", "application/json")
                    .header("Accept", "application/json")
                    .json(&payload)
                    .send()
                    .await
                    .with_context(|| format!("Could not call function: {}", payload))?;
            }
            Err(e) => {
                tracing::error!(
                    "Could not login to FreeIPA server: {}. Error: {}",
                    auth.server,
                    e
                );
            }
        }
    }

    // reset the number of reconnects, as we have clearly been successful
    auth.num_reconnects = 0;

    if result.status().is_success() {
        let result = result
            .json::<FreeResponse>()
            .await
            .context("Could not decode from json")?;

        // if there is an error, return it
        if !result.error.is_null() {
            return Err(Error::Call(format!(
                "Error in response: {:?}",
                result.error
            )));
        }

        // return the result, encoded to the type T
        match serde_json::from_value(result.result.clone()).context("Could not decode result") {
            Ok(result) => Ok(result),
            Err(e) => {
                tracing::error!("Could not decode result: {:?}. Error: {}", result.result, e);
                tracing::error!("Response: {:?}", result);
                Err(Error::Call(format!(
                    "Could not decode result: {:?}. Error: {}",
                    result.result, e
                )))
            }
        }
    } else {
        tracing::error!(
            "Could not get response for function: {}. Status: {}. Response: {:?}",
            url,
            result.status(),
            result
        );
        Err(Error::Call(format!(
            "Could not get response for function: {}. Status: {}. Response: {:?}",
            payload,
            result.status(),
            result
        )))
    }
}

#[derive(Debug, Clone)]
struct SlurmAuth {
    server: String,
    token_command: String,
    user: String,
    jwt: Secret<String>,
    num_reconnects: u32,
}

impl SlurmAuth {
    fn default() -> Self {
        SlurmAuth {
            server: "".to_string(),
            token_command: "".to_string(),
            user: "".to_string(),
            jwt: Secret::new("".to_string()),
            num_reconnects: 0,
        }
    }
}

static SLURM_AUTH: Lazy<Mutex<SlurmAuth>> = Lazy::new(|| Mutex::new(SlurmAuth::default()));

///
/// Login to the Slurm server using the passed passed command to generate
/// the JWT token. This will return the valid JWT in a secret. This
/// JWT can be used for subsequent calls to the server.
///
async fn login(server: &str, user: &str, token_command: &str) -> Result<Secret<String>, Error> {
    // for now just use the token_command as the token...

    // get the JWT token via a tokio process
    /* let jwt = tokio::process::Command::new(token_command)
        .output()
        .await
        .with_context(|| "Could not get JWT token")?;

    let jwt = String::from_utf8(jwt.stdout).context("Could not convert JWT to string")?; */

    let jwt = token_command.to_string();

    // create a client
    let client = Client::builder()
        .build()
        .context("Could not build client")?;

    // first we need to find the version of the API provided by the
    // server. This is done by looking at the /openapi.json file
    // and parsing what we find there.
    let url = format!("{}/openapi.json", server);

    let result = client
        .get(&url)
        .header("Accept", "application/json")
        .header("X-SLURM-USER-NAME", user)
        .header("X-SLURM-USER-TOKEN", jwt.clone())
        .send()
        .await
        .with_context(|| format!("Could not login calling URL: {}", url))?;

    tracing::info!("Login result: {:?}", result);
    match &result.json::<serde_json::Value>().await {
        Ok(json) => {
            tracing::info!("Login JSON: {:?}", json);
        }
        Err(e) => {
            tracing::error!("Could not decode JSON: {}", e);
        }
    }

    /*match result.status() {
        status if status.is_success() => Ok(Secret::new(jwt.clone())),
        _ => Err(Error::Login(format!(
            "Could not login to server: {}. Status: {}. Response: {:?}",
            server,
            result.status(),
            result
        ))),
    }*/

    Ok(Secret::new(jwt.clone()))
}

// function to return the client protected by a MutexGuard
async fn auth<'mg>() -> Result<MutexGuard<'mg, SlurmAuth>, Error> {
    Ok(SLURM_AUTH.lock().await)
}

///
/// Public API
///

pub async fn connect(server: &str, user: &str, token_command: &str) -> Result<(), Error> {
    // overwrite the global FreeIPA client with a new one
    let mut auth = SLURM_AUTH.lock().await;

    auth.server = server.to_string();
    auth.user = user.to_string();
    auth.token_command = token_command.to_string();
    auth.jwt = Secret::new("".to_string());
    auth.num_reconnects = 0;

    const MAX_RECONNECTS: u32 = 3;
    const RECONNECT_WAIT: u64 = 100;

    loop {
        match login(&auth.server, &auth.user, &auth.token_command).await {
            Ok(jwt) => {
                auth.jwt = jwt;
                return Ok(());
            }
            Err(e) => {
                tracing::error!("Could not login to Slurm server: {}. Error: {}", server, e);

                auth.num_reconnects += 1;

                if auth.num_reconnects > MAX_RECONNECTS {
                    return Err(Error::Login(format!(
                        "Could not login to Slurm server: {}. Error: {}",
                        server, e
                    )));
                }

                tokio::time::sleep(std::time::Duration::from_millis(RECONNECT_WAIT)).await;
            }
        }
    }
}
