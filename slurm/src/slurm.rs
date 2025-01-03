// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use anyhow::Context;
use anyhow::Result;
use once_cell::sync::Lazy;
use reqwest::{Client, Url};
use secrecy::{ExposeSecret, Secret};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::fmt::Display;
use templemeads::grammar::{ProjectMapping, UserMapping};
use templemeads::Error;
use tokio::sync::{Mutex, MutexGuard};

use crate::cache;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct FreeResponse {
    meta: serde_json::Value,
    errors: serde_json::Value,
    warnings: serde_json::Value,
}

///
/// Call a get URL on the slurmrestd server described in 'auth'.
///
async fn call_get(
    backend: &str,
    function: &str,
    query_params: &Vec<(&str, &str)>,
) -> Result<serde_json::Value, Error> {
    // get the auth details from the global Slurm client
    let mut auth = auth().await?;
    auth.num_reconnects = 0;

    // has the token expired?
    if auth.token_expired()? {
        tracing::warn!("Token has expired. Reconnecting.");

        // try to reconnect to the server
        loop {
            match login(
                &auth.server,
                &auth.user,
                &auth.token_command,
                auth.token_lifespan,
            )
            .await
            {
                Ok(session) => {
                    auth.jwt = session.jwt;
                    auth.jwt_creation_time = session.start_time;
                    auth.version = session.version;
                    break;
                }
                Err(e) => {
                    tracing::error!(
                        "Could not login to FreeIPA server: {}. Error: {}",
                        auth.server,
                        e
                    );

                    auth.num_reconnects += 1;

                    if auth.num_reconnects > 3 {
                        auth.num_reconnects = 0;
                        return Err(Error::Call(
                            "Failed multiple reconnection attempts!".to_string(),
                        ));
                    }
                }
            }

            // sleep for 100 ms before trying again
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }

        auth.num_reconnects = 0;
    }

    let url = match Url::parse_with_params(
        &format!(
            "{}/{}/v{}/{}",
            &auth.server, backend, &auth.version, function
        ),
        query_params,
    ) {
        Ok(url) => url,
        Err(e) => {
            tracing::error!("Could not parse URL: {}", e);
            return Err(Error::Call("Could not parse URL".to_string()));
        }
    };

    tracing::info!("Calling function {}", url);

    let client = Client::builder()
        .build()
        .context("Could not build client")?;

    let mut result = client
        .get(url.clone())
        .header("Referer", format!("{}/ipa", &auth.server))
        .header("Content-Type", "application/json")
        .header("Accept", "application/json")
        .header("X-SLURM-USER-NAME", &auth.user)
        .header("X-SLURM-USER-TOKEN", auth.jwt.expose_secret().to_string())
        .send()
        .await
        .with_context(|| format!("Could not call function: {}", url))?;

    // if this is an authorisation error, try to reconnect
    while result.status().as_u16() == 401 {
        auth.num_reconnects += 1;

        if auth.num_reconnects > 3 {
            return Err(Error::Call(format!(
                "Authorisation (401) error: Could not get response for function: {}. Status: {}. Response: {:?}",
                url,
                result.status(),
                    result
                )));
        }

        tracing::error!("Authorisation (401) error. Reconnecting.");

        match login(
            &auth.server,
            &auth.user,
            &auth.token_command,
            auth.token_lifespan,
        )
        .await
        {
            Ok(session) => {
                auth.jwt = session.jwt;
                auth.jwt_creation_time = session.start_time;
                auth.version = session.version;

                // create a new client with the new cookies
                let client = Client::builder()
                    .build()
                    .context("Could not build client")?;

                // retry the call
                result = client
                    .get(url.clone())
                    .header("Referer", format!("{}/ipa", &auth.server))
                    .header("Content-Type", "application/json")
                    .header("Accept", "application/json")
                    .send()
                    .await
                    .with_context(|| format!("Could not call function: {}", url))?;
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

    if result.status().as_u16() == 500 {
        tracing::error!(
            "500 error - slurmrestd error when calling {} as user {}.",
            url,
            auth.user
        );

        match result.json::<serde_json::Value>().await {
            Ok(json) => tracing::error!("Server response: {}", json),
            Err(_) => tracing::error!("Could not decode the server's response."),
        };

        return Err(Error::Call(format!(
            "500 error - slurmrestd error when calling {} as user {}.",
            url, auth.user
        )));
    }

    // reset the number of reconnects, as we have clearly been successful
    auth.num_reconnects = 0;

    if result.status().is_success() {
        let response: serde_json::Value = result
            .json()
            .await
            .with_context(|| "Could not decode json from response".to_owned())?;

        // are there any warnings - print them out if there are
        if let Some(warnings) = response
            .get("warnings")
            .unwrap_or(&serde_json::Value::Null)
            .as_array()
        {
            if !warnings.is_empty() {
                tracing::warn!("Warnings: {:?}", warnings);
            }
        }

        // are there any errors - raise these as errors if there are
        if let Some(errors) = response
            .get("errors")
            .unwrap_or(&serde_json::Value::Null)
            .as_array()
        {
            if !errors.is_empty() {
                tracing::error!("Errors: {:?}", errors);
                return Err(Error::Call(format!("Slurmrestd errors: {:?}", errors)));
            }
        }

        Ok(response)
    } else {
        tracing::error!(
            "Could not get response for function: {}. Status: {}. Response: {:?}",
            url,
            result.status(),
            result
        );
        Err(Error::Call(format!(
            "Could not get response for function: {}. Status: {}. Response: {:?}",
            url,
            result.status(),
            result
        )))
    }
}

///
/// Call a post URL on the slurmrestd server described in 'auth'.
///
async fn call_post(
    backend: &str,
    function: &str,
    payload: &serde_json::Value,
) -> Result<(), Error> {
    // get the auth details from the global Slurm client
    let mut auth = auth().await?;
    auth.num_reconnects = 0;

    // has the token expired?
    if auth.token_expired()? {
        tracing::warn!("Token has expired. Reconnecting.");

        // try to reconnect to the server
        loop {
            match login(
                &auth.server,
                &auth.user,
                &auth.token_command,
                auth.token_lifespan,
            )
            .await
            {
                Ok(session) => {
                    auth.jwt = session.jwt;
                    auth.jwt_creation_time = session.start_time;
                    auth.version = session.version;
                    break;
                }
                Err(e) => {
                    tracing::error!(
                        "Could not login to FreeIPA server: {}. Error: {}",
                        auth.server,
                        e
                    );

                    auth.num_reconnects += 1;

                    if auth.num_reconnects > 3 {
                        auth.num_reconnects = 0;
                        return Err(Error::Call(
                            "Failed multiple reconnection attempts!".to_string(),
                        ));
                    }
                }
            }

            // sleep for 100 ms before trying again
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }

        auth.num_reconnects = 0;
    }

    let url = format!(
        "{}/{}/v{}/{}",
        &auth.server, backend, &auth.version, function
    );

    tracing::info!("Calling function {} with payload: {:?}", url, payload);

    let client = Client::builder()
        .build()
        .context("Could not build client")?;

    let mut result = client
        .post(&url)
        .header("Referer", format!("{}/ipa", &auth.server))
        .header("Content-Type", "application/json")
        .header("Accept", "application/json")
        .header("X-SLURM-USER-NAME", &auth.user)
        .header("X-SLURM-USER-TOKEN", auth.jwt.expose_secret().to_string())
        .json(&payload)
        .send()
        .await
        .with_context(|| format!("Could not call function: {}", url))?;

    // if this is an authorisation error, try to reconnect
    while result.status().as_u16() == 401 {
        auth.num_reconnects += 1;

        if auth.num_reconnects > 3 {
            return Err(Error::Call(format!(
                "Authorisation (401) error: Could not get response for function: {}. Status: {}. Response: {:?}",
                url,
                result.status(),
                    result
                )));
        }

        tracing::error!("Authorisation (401) error. Reconnecting.");

        match login(
            &auth.server,
            &auth.user,
            &auth.token_command,
            auth.token_lifespan,
        )
        .await
        {
            Ok(session) => {
                auth.jwt = session.jwt;
                auth.jwt_creation_time = session.start_time;
                auth.version = session.version;

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
                    .with_context(|| format!("Could not call function: {}", url))?;
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

    if result.status().as_u16() == 500 {
        tracing::error!(
            "500 error - slurmrestd error when calling {} with payload {} as user {}.",
            url,
            payload,
            auth.user
        );

        match result.json::<serde_json::Value>().await {
            Ok(json) => tracing::error!("Server response: {}", json),
            Err(_) => tracing::error!("Could not decode the server's response."),
        };

        return Err(Error::Call(format!(
            "500 error - slurmrestd error when calling {} with payload {} as user {}.",
            url, payload, auth.user
        )));
    }

    if result.status().as_u16() == 304 {
        // this is returned when the post causes no change on the server
        tracing::warn!(
            "Server returned '304'. No change for function: {} with payload {:?}",
            url,
            payload
        );

        return Ok(());
    }

    // reset the number of reconnects, as we have clearly been successful
    auth.num_reconnects = 0;

    if result.status().is_success() {
        let response: serde_json::Value = result
            .json()
            .await
            .with_context(|| "Could not decode json from response".to_owned())?;

        // are there any warnings - print them out if there are
        if let Some(warnings) = response
            .get("warnings")
            .unwrap_or(&serde_json::Value::Null)
            .as_array()
        {
            if !warnings.is_empty() {
                tracing::warn!("Warnings: {:?}", warnings);
            }
        }

        // are there any errors - raise these as errors if there are
        if let Some(errors) = response
            .get("errors")
            .unwrap_or(&serde_json::Value::Null)
            .as_array()
        {
            if !errors.is_empty() {
                tracing::error!("Errors: {:?}", errors);
                return Err(Error::Call(format!("Slurmrestd errors: {:?}", errors)));
            }
        }

        Ok(())
    } else {
        tracing::error!(
            "Could not get response for function: {}. Status: {}. Response: {:?}",
            url,
            result.status(),
            result
        );
        Err(Error::Call(format!(
            "Could not get response for function: {}. Status: {}. Response: {:?}",
            url,
            result.status(),
            result
        )))
    }
}

#[derive(Debug, Clone)]
struct SlurmAuth {
    server: String,
    token_command: String,
    token_lifespan: u32,
    user: String,
    jwt: Secret<String>,
    jwt_creation_time: u64,
    version: String,
    num_reconnects: u32,
}

impl SlurmAuth {
    fn default() -> Self {
        SlurmAuth {
            server: "".to_string(),
            token_command: "".to_string(),
            token_lifespan: 1800,
            user: "".to_string(),
            jwt: Secret::new("".to_string()),
            jwt_creation_time: 0,
            version: "".to_string(),
            num_reconnects: 0,
        }
    }

    fn token_expired(&self) -> Result<bool, Error> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .context("Could not get current time")?;

        // we give ourselves a 10 second margin of error
        Ok(10 + now.as_secs() - self.jwt_creation_time > self.token_lifespan as u64)
    }
}

static SLURM_AUTH: Lazy<Mutex<SlurmAuth>> = Lazy::new(|| Mutex::new(SlurmAuth::default()));

struct SlurmSession {
    jwt: Secret<String>,
    version: String,
    start_time: u64,
}

///
/// Login to the Slurm server using the passed passed command to generate
/// the JWT token. This will return the valid JWT in a secret. This
/// JWT can be used for subsequent calls to the server.
///
async fn login(
    server: &str,
    user: &str,
    token_command: &str,
    token_lifespan: u32,
) -> Result<SlurmSession, Error> {
    tracing::info!("Logging into Slurm server: {} using user {}", server, user);

    let mut token_command = token_command.to_string();

    // find out the unix user that is running this process
    let process_user = whoami::username();

    if process_user != user {
        tracing::info!(
            "Token is for user '{}', but process is running as '{}'",
            user,
            process_user
        );

        // This is a different user - make sure to add 'username=user' to the token command
        token_command = format!("{} username={}", token_command, user);
    }

    // add on the lifespan to the token command
    token_command = format!("{} lifespan={}", token_command, token_lifespan);

    tracing::info!("Getting JWT token from command: {}", token_command);

    // parse 'token_command' into an executable plus arguments
    let token_command = shlex::split(&token_command).context("Could not parse token command")?;

    let token_exe = token_command.first().context("No token command")?;
    let token_args = token_command.get(1..).unwrap_or(&[]);

    // get the current datetime in seconds since the epoch - we will use this
    // to check the token expiry
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .context("Could not get current time")?;

    // get the JWT token via a tokio process
    let jwt = match tokio::process::Command::new(token_exe)
        .args(token_args)
        .output()
        .await
    {
        Ok(jwt) => String::from_utf8(jwt.stdout).context("Could not convert JWT to string")?,
        Err(e) => {
            tracing::error!(
                "Could not get JWT token using command '{:?}': {}",
                token_command,
                e
            );
            return Err(Error::Login("Could not get JWT token".to_string()));
        }
    };

    // we expect the output to be something like "JWT: SLURM_JWT={TOKEN}"
    // We will split with spaces, then find the work that is '{something}={token}",
    // then split this with '=' and take the second part.
    let jwt = jwt
        .split_whitespace()
        .find(|x| x.contains("="))
        .context(format!("Could not find JWT token from '{}'", jwt))?
        .split('=')
        .nth(1)
        .context(format!("Could not extract JWT token from '{}'", jwt))?
        .to_string();

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
        .with_context(|| format!("Could not get OpenAPI specification calling URL: {}", url))?;

    // convert the response to JSON
    let openapi_spec = match &result.json::<serde_json::Value>().await {
        Ok(json) => json.clone(),
        Err(e) => {
            tracing::error!("Could not decode JSON: {}", e);
            return Err(Error::Login(format!(
                "Could not decode JSON from OpenAPI specification: {}",
                e
            )));
        }
    };

    // there should be a 'info' section in the openapi spec
    let info = openapi_spec
        .get("info")
        .context("Could not find 'info' section in OpenAPI specification")?;

    // the version is in the 'version' field
    let version = info
        .get("version")
        .context("Could not find 'version' field in OpenAPI specification")?;

    tracing::info!("Slurm OpenAPI version: {}", version);

    // the version number has the format 'dbvX.Y.Z`. We need to extract
    // the X.Y.Z part.
    let version = version
        .as_str()
        .context("Could not convert version to string")?
        .split('v')
        .nth(1)
        .context("Could not split version")?;

    // sometimes there is an additional '&something' afterwards - remove it
    let version = version
        .split('&')
        .next()
        .context("Could not split version")?
        .to_string();

    // extract the version string above into major.minor.patch numbers
    let mut version_numbers: Vec<u32> = version
        .split('.')
        .map(|x| x.parse::<u32>())
        .collect::<Result<Vec<u32>, _>>()
        .context("Could not parse version numbers")?;

    let mut working_version = None;

    // the Slurm API supports normally 3 versions - this has reported the
    // lowest version - see how many higher versions we can use
    tracing::info!("Auto detecting maximum version of the Slurm API...");
    loop {
        // create a test version by joining together the version numbers as strings
        let test_version: String = version_numbers
            .iter()
            .map(|x| x.to_string())
            .collect::<Vec<String>>()
            .join(".");
        tracing::info!("Testing version {}", test_version);

        // call the ping function to make sure that the server is
        // up and running
        let url = format!("{}/slurm/v{}/ping", server, test_version);

        let result = match client
            .get(&url)
            .header("Accept", "application/json")
            .header("X-SLURM-USER-NAME", user)
            .header("X-SLURM-USER-TOKEN", jwt.clone())
            .send()
            .await
        {
            Ok(result) => result,
            Err(e) => {
                tracing::error!("Version {} is not supported. {}", test_version, e);
                break;
            }
        };

        // convert the response to JSON
        let ping_response = match &result.json::<serde_json::Value>().await {
            Ok(json) => json.clone(),
            Err(e) => {
                tracing::warn!(
                    "Could not decode JSON - version {} is not supported: {}",
                    e,
                    test_version
                );
                break;
            }
        };

        tracing::info!("Ping response: {:?}", ping_response);
        working_version = Some(test_version);
        version_numbers[2] += 1;
    }

    let version = match working_version {
        Some(version) => version,
        None => {
            return Err(Error::Login(
                "Could not find a working version of the Slurm API".to_string(),
            ));
        }
    };

    tracing::info!("Using version {} of the Slurm API", version);

    // now we have connected, we need to find the cluster that we
    // should be working on
    let result = client
        .get(format!("{}/slurmdb/v{}/clusters", server, version))
        .header("Accept", "application/json")
        .header("X-SLURM-USER-NAME", user)
        .header("X-SLURM-USER-TOKEN", jwt.clone())
        .send()
        .await
        .with_context(|| "Could not get cluster information")?;

    let clusters = match &result.json::<serde_json::Value>().await {
        Ok(json) => json.clone(),
        Err(e) => {
            tracing::error!("Could not decode JSON: {}", e);
            return Err(Error::Login("Could not decode JSON".to_string()));
        }
    };

    // there should be an array of cluster objects, each with a `name` field.
    // Get all of the cluster names.
    let clusters = match clusters.get("clusters") {
        Some(clusters) => match clusters.as_array() {
            Some(clusters) => {
                let clusters: Vec<String> = clusters
                    .iter()
                    .map(|c| {
                        c.get("name")
                            .and_then(|n| n.as_str())
                            .unwrap_or("unknown")
                            .to_string()
                    })
                    .collect();

                tracing::info!("Clusters: {:?}", clusters);

                if clusters.is_empty() {
                    tracing::error!("No clusters found in response: {:?}", clusters);
                    return Err(Error::Login("No clusters found".to_string()));
                }

                clusters
            }
            None => {
                tracing::error!("Clusters is not an array: {:?}", clusters);
                return Err(Error::Login("Clusters is not an array".to_string()));
            }
        },
        None => {
            tracing::error!("Could not get clusters from response: {:?}", clusters);
            return Err(Error::Login(
                "Could not get clusters from response".to_string(),
            ));
        }
    };

    // now get the requested cluster from the cache
    let requested_cluster = cache::get_option_cluster().await?;

    if let Some(requested_cluster) = requested_cluster {
        if clusters.contains(&requested_cluster) {
            tracing::info!("Using requested cluster: {}", requested_cluster);
        } else {
            tracing::warn!(
                "Requested cluster {} not found in list of clusters: {:?}",
                requested_cluster,
                clusters
            );
            return Err(Error::Login("Requested cluster not found".to_string()));
        }
    } else {
        tracing::info!(
            "Using the first cluster available by default: {}",
            clusters[0]
        );
        cache::set_cluster(&clusters[0]).await?;
    }

    Ok(SlurmSession {
        jwt: Secret::new(jwt),
        version: version.to_string(),
        start_time: now.as_secs(),
    })
}

// function to return the client protected by a MutexGuard
async fn auth<'mg>() -> Result<MutexGuard<'mg, SlurmAuth>, Error> {
    Ok(SLURM_AUTH.lock().await)
}

async fn force_add_slurm_account(account: &SlurmAccount) -> Result<SlurmAccount, Error> {
    // need to POST to /slurm/vX.Y.Z/accounts, using a JSON content
    // with
    // {
    //    accounts: [
    //        {
    //            name: "project",
    //            description: "Account for project"
    //            organization: "openportal"
    //        }
    //    ]
    // }

    if account.organization() != get_managed_organization() {
        tracing::warn!(
            "Account {} is not managed by the openportal organization - we cannot manage it.",
            account
        );
        return Err(Error::UnmanagedGroup(format!(
            "Cannot add Slurm account as {} is not managed by openportal",
            account
        )));
    }

    let payload = serde_json::json!({
        "accounts": [
            {
                "name": account.name,
                "description": account.description,
                "organization": account.organization
            }
        ]
    });

    call_post("slurmdb", "accounts", &payload).await?;

    Ok(account.clone())
}

async fn get_account_from_slurm(account: &str) -> Result<Option<SlurmAccount>, Error> {
    let account = clean_account_name(account)?;

    let response = match call_get("slurmdb", &format!("account/{}", account), &Vec::new()).await {
        Ok(response) => response,
        Err(e) => {
            tracing::warn!("Could not get account {}: {}", account, e);
            return Ok(None);
        }
    };

    // there should be an accounts list, with a single entry for this account
    let accounts = match response.get("accounts") {
        Some(accounts) => accounts,
        None => {
            tracing::warn!("Could not get accounts from response: {:?}", response);
            return Ok(None);
        }
    };

    // this should be an array
    let accounts = match accounts.as_array() {
        Some(accounts) => accounts,
        None => {
            tracing::warn!("Accounts is not an array: {:?}", accounts);
            return Ok(None);
        }
    };

    // there should be an Account object in this array with the right name
    let slurm_account = accounts.iter().find(|a| {
        let name = a.get("name").and_then(|n| n.as_str());
        name == Some(&account)
    });

    let account = match slurm_account {
        Some(account) => account,
        None => {
            tracing::warn!(
                "Could not find account '{}' in response: {:?}",
                account,
                response
            );
            return Ok(None);
        }
    };

    match SlurmAccount::construct(account) {
        Ok(account) => Ok(Some(account)),
        Err(e) => {
            tracing::warn!("Could not construct account from response: {}", e);
            Ok(None)
        }
    }
}

async fn get_account(account: &str) -> Result<Option<SlurmAccount>, Error> {
    // need to GET /slurm/vX.Y.Z/accounts/{account.name}
    // and return the account if it exists
    let cached_account = cache::get_account(account).await?;

    if let Some(cached_account) = cached_account {
        // double-check that the account actually exists...
        let existing_account = match get_account_from_slurm(cached_account.name()).await {
            Ok(account) => account,
            Err(e) => {
                tracing::warn!("Could not get account {}: {}", cached_account.name(), e);
                cache::clear().await?;
                return Ok(None);
            }
        };

        if let Some(existing_account) = existing_account {
            if cached_account != existing_account {
                tracing::warn!(
                    "Account {} exists, but with different details.",
                    cached_account.name()
                );
                tracing::warn!(
                    "Existing: {:?}, new: {:?}",
                    existing_account,
                    cached_account
                );

                // clear the cache as something has changed behind our back
                cache::clear().await?;

                // store the new account
                cache::add_account(&existing_account).await?;

                return Ok(Some(existing_account));
            } else {
                return Ok(Some(cached_account));
            }
        } else {
            // the account doesn't exist
            tracing::warn!(
                "Account {} does not exist - it has been removed from slurm.",
                cached_account.name()
            );
            cache::clear().await?;
            return Ok(None);
        }
    }

    // see if we can read the account from slurm
    let account = match get_account_from_slurm(account).await {
        Ok(account) => account,
        Err(e) => {
            tracing::warn!("Could not get account {}: {}", account, e);
            return Ok(None);
        }
    };

    if let Some(account) = account {
        cache::add_account(&account).await?;
        Ok(Some(account))
    } else {
        Ok(None)
    }
}

async fn get_account_create_if_not_exists(account: &SlurmAccount) -> Result<SlurmAccount, Error> {
    let existing_account = get_account(account.name()).await?;

    if let Some(existing_account) = existing_account {
        if account.organization() != get_managed_organization() {
            tracing::warn!(
                "Account {} is not managed by the openportal organization - we cannot manage it.",
                account
            );
            return Err(Error::UnmanagedGroup(format!(
                "Cannot add Slurm account as {} is not managed by openportal",
                account
            )));
        }

        if existing_account.description() != account.description()
            || existing_account.organization() != account.organization()
        {
            // the account exists, but the details are different
            tracing::warn!(
                "Account {} exists, but with different details.",
                account.name()
            );
            tracing::warn!("Existing: {:?}, new: {:?}", existing_account, account)
        }

        tracing::info!("Using existing slurm account {}", existing_account);
        return Ok(existing_account);
    }

    // it doesn't, so create it
    tracing::info!("Creating new slurm account: {}", account.name());
    let account = force_add_slurm_account(account).await?;
    cache::add_account(&account).await?;

    Ok(account.clone())
}

async fn get_user_from_slurm(user: &str) -> Result<Option<SlurmUser>, Error> {
    let user = clean_user_name(user)?;

    let query_params = vec![("with_assocs", "true"), ("default_account", "true")];

    let response = match call_get("slurmdb", &format!("user/{}", user), &query_params).await {
        Ok(response) => response,
        Err(e) => {
            tracing::warn!("Could not get user {}: {}", user, e);
            return Ok(None);
        }
    };

    // there should be a users list, with a single entry for this user
    let users = match response.get("users") {
        Some(users) => users,
        None => {
            tracing::warn!("Could not get users from response: {:?}", response);
            return Ok(None);
        }
    };

    // this should be an array
    let users = match users.as_array() {
        Some(users) => users,
        None => {
            tracing::warn!("Users is not an array: {:?}", users);
            return Ok(None);
        }
    };

    // there should be an User object in this array with the right name
    let slurm_user = users.iter().find(|u| {
        let name = u.get("name").and_then(|n| n.as_str());
        name == Some(&user)
    });

    let user = match slurm_user {
        Some(user) => user,
        None => {
            tracing::warn!("Could not find user '{}' in response: {:?}", user, response);
            return Ok(None);
        }
    };

    match SlurmUser::construct(user) {
        Ok(user) => Ok(Some(user)),
        Err(e) => {
            tracing::warn!("Could not construct user from response: {}", e);
            Ok(None)
        }
    }
}

async fn get_user(user: &str) -> Result<Option<SlurmUser>, Error> {
    let cached_user = cache::get_user(user).await?;

    if let Some(cached_user) = cached_user {
        // double-check that the user actually exists...
        let existing_user = match get_user_from_slurm(cached_user.name()).await {
            Ok(user) => user,
            Err(e) => {
                tracing::warn!("Could not get user {}: {}", cached_user.name(), e);
                cache::clear().await?;
                return Ok(None);
            }
        };

        if let Some(existing_user) = existing_user {
            if cached_user != existing_user {
                tracing::warn!(
                    "User {} exists, but with different details.",
                    cached_user.name()
                );
                tracing::warn!("Existing: {:?}, new: {:?}", existing_user, cached_user);

                // clear the cache as something has changed behind our back
                cache::clear().await?;

                // store the new user
                cache::add_user(&existing_user).await?;

                return Ok(Some(existing_user));
            } else {
                return Ok(Some(cached_user));
            }
        } else {
            // the user doesn't exist
            tracing::warn!(
                "User {} does not exist - it has been removed from slurm.",
                cached_user.name()
            );
            cache::clear().await?;
            return Ok(None);
        }
    }

    // see if we can read the user from slurm
    let user = match get_user_from_slurm(user).await {
        Ok(user) => user,
        Err(e) => {
            tracing::warn!("Could not get user {}: {}", user, e);
            return Ok(None);
        }
    };

    if let Some(user) = user {
        cache::add_user(&user).await?;
        Ok(Some(user))
    } else {
        Ok(None)
    }
}

async fn add_account_association(account: &SlurmAccount) -> Result<(), Error> {
    // eventually should check to see if this association already exists,
    // and if so, not to do anything else

    if account.organization() != get_managed_organization() {
        tracing::warn!(
            "Account {} is not managed by the openportal organization - we cannot manage it.",
            account
        );
        return Err(Error::UnmanagedGroup(format!(
            "Cannot add Slurm account as {} is not managed by openportal",
            account
        )));
    }

    // get the cluster name from the cache
    let cluster = cache::get_cluster().await?;

    // add the association condition to the account
    let payload = serde_json::json!({
        "association_condition": {
            "accounts": [account.name],
            "clusters": [cluster],
            "association": {
                "defaultqos": "normal",
                "comment": format!("Association added by OpenPortal for account {}", account.name)
            }
        }
    });

    call_post("slurmdb", "accounts_association", &payload).await?;

    Ok(())
}

async fn add_user_association(
    user: &SlurmUser,
    account: &SlurmAccount,
    make_default: bool,
) -> Result<SlurmUser, Error> {
    if account.organization() != get_managed_organization() {
        tracing::warn!(
            "Account {} is not managed by the openportal organization - we cannot manage it.",
            account
        );
        return Err(Error::UnmanagedGroup(format!(
            "Cannot add Slurm account as {} is not managed by openportal",
            account
        )));
    }

    let mut user = user.clone();
    let mut user_changed = false;

    // first, add the association if it doesn't exist
    if !user
        .associations()
        .iter()
        .any(|a| a.account() == account.name())
    {
        // make sure that we have this association on the account
        add_account_association(account).await?;

        // now add the association to the user
        let payload = serde_json::json!({
            "associations": [
                {
                    "user": user.name,
                    "account": account.name,
                    "comment": format!("Association added by OpenPortal between user {} and account {}",
                                       user.name, account.name),
                    "cluster": "linux",
                    "is_default": true
                }
            ]
        });

        call_post("slurmdb", "associations", &payload).await?;

        // update the user
        user = match get_user_from_slurm(user.name()).await? {
            Some(user) => user,
            None => {
                return Err(Error::Call(format!(
                    "Could not get user that just had its associations updated! '{}'",
                    user.name()
                )))
            }
        };

        user_changed = true;

        tracing::info!("Updated user: {}", user);
    }

    if make_default && *user.default_account() != Some(account.name().to_string()) {
        let payload = serde_json::json!({
            "users": [
                {
                    "name": user.name,
                    "default": {
                        "account": account.name
                    }
                }
            ]
        });

        call_post("slurmdb", "users", &payload).await?;

        // update the user
        user = match get_user_from_slurm(user.name()).await? {
            Some(user) => user,
            None => {
                return Err(Error::Call(format!(
                    "Could not get user that just had its default account updated! '{}'",
                    user.name()
                )))
            }
        };

        user_changed = true;
    }

    if user_changed {
        // now cache the updated user
        cache::add_user(&user).await?;
    } else {
        tracing::info!("Using existing user: {}", user);
    }

    Ok(user)
}

async fn get_user_create_if_not_exists(user: &UserMapping) -> Result<SlurmUser, Error> {
    // first, make sure that the account exists
    let slurm_account =
        get_account_create_if_not_exists(&SlurmAccount::from_mapping(&user.clone().into())?)
            .await?;

    // now get the user from slurm
    let slurm_user = get_user(user.local_user()).await?;

    if let Some(slurm_user) = slurm_user {
        // the user exists - check that the account is associated with the user
        if *slurm_user.default_account() == Some(slurm_account.name().to_string())
            && slurm_user
                .associations()
                .iter()
                .any(|a| a.account() == slurm_account.name())
        {
            tracing::info!("Using existing user {}", slurm_user);
            return Ok(slurm_user);
        } else {
            tracing::warn!(
                "User {} exists, but is not default associated with the requested account '{}'.",
                user,
                slurm_account
            );
        }
    }

    // first, create the user
    let username = clean_user_name(user.local_user())?;

    let payload = serde_json::json!({
        "users": [
            {
                "name": username,
            }
        ]
    });

    call_post("slurmdb", "users", &payload).await?;

    // now load the user from slurm to make sure it exists
    let slurm_user = match get_user(user.local_user()).await? {
        Some(user) => user,
        None => {
            return Err(Error::Call(format!(
                "Could not get user that was just created! '{}'",
                user.local_user()
            )))
        }
    };

    // now add the association to the account, making it the default
    let slurm_user = add_user_association(&slurm_user, &slurm_account, true).await?;

    let user = SlurmUser::from_mapping(user)?;

    // check we have the user we expected
    if slurm_user != user {
        tracing::warn!("User {} exists, but with different details.", user.name());
        tracing::warn!("Existing: {:?}, new: {:?}", slurm_user, user);
    }

    Ok(slurm_user)
}

///
/// Public API
///

///
/// Return the organization that indicates that this user / account is managed
///
pub fn get_managed_organization() -> String {
    "openportal".to_string()
}

pub fn clean_account_name(account: &str) -> Result<String, Error> {
    let account = account.trim();

    if account.is_empty() {
        return Err(Error::Call("Account name is empty".to_string()));
    }

    Ok(account
        .replace("/", "_")
        .replace(" ", "_")
        .to_ascii_lowercase())
}

pub fn clean_user_name(user: &str) -> Result<String, Error> {
    let user = user.trim();

    if user.is_empty() {
        return Err(Error::Call("User name is empty".to_string()));
    }

    Ok(user
        .replace("/", "_")
        .replace(" ", "_")
        .to_ascii_lowercase())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SlurmAccount {
    name: String,
    description: String,
    organization: String,
}

impl PartialEq for SlurmAccount {
    fn eq(&self, other: &Self) -> bool {
        self.name == other.name
            && self.organization == other.organization
            && self.description.to_ascii_lowercase() == other.description.to_ascii_lowercase()
    }
}

impl Display for SlurmAccount {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "SlurmAccount {{ name: {}, description: {}, organization: {} }}",
            self.name(),
            self.description(),
            self.organization()
        )
    }
}

impl SlurmAccount {
    pub fn from_mapping(mapping: &ProjectMapping) -> Result<Self, Error> {
        Ok(SlurmAccount {
            name: clean_account_name(mapping.local_group())?,
            description: format!("Account for OpenPortal project {}", mapping.project()),
            organization: get_managed_organization(),
        })
    }

    pub fn construct(result: &serde_json::Value) -> Result<Self, Error> {
        let name = match result.get("name") {
            Some(name) => name,
            None => {
                tracing::warn!("Could not get name from account: {:?}", result);
                return Err(Error::Call("Could not get name from account".to_string()));
            }
        };

        let name = match name.as_str() {
            Some(name) => name,
            None => {
                tracing::warn!("Could not get name as string from account: {:?}", name);
                return Err(Error::Call(
                    "Could not get name as string from account".to_string(),
                ));
            }
        };

        let description = match result.get("description") {
            Some(description) => description,
            None => {
                tracing::warn!("Could not get description from account: {:?}", result);
                return Err(Error::Call(
                    "Could not get description from account".to_string(),
                ));
            }
        };

        let description = match description.as_str() {
            Some(description) => description,
            None => {
                tracing::warn!(
                    "Could not get description as string from account: {:?}",
                    description
                );
                return Err(Error::Call(
                    "Could not get description as string from account".to_string(),
                ));
            }
        };

        let organization = match result.get("organization") {
            Some(organization) => organization,
            None => {
                tracing::warn!("Could not get organization from account: {:?}", result);
                return Err(Error::Call(
                    "Could not get organization from account".to_string(),
                ));
            }
        };

        let organization = match organization.as_str() {
            Some(organization) => organization,
            None => {
                tracing::warn!(
                    "Could not get organization as string from account: {:?}",
                    organization
                );
                return Err(Error::Call(
                    "Could not get organization as string from account".to_string(),
                ));
            }
        };

        Ok(SlurmAccount {
            name: clean_account_name(name)?,
            description: description.to_string(),
            organization: organization.to_string(),
        })
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn description(&self) -> &str {
        &self.description
    }

    pub fn organization(&self) -> &str {
        &self.organization
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SlurmAssociation {
    user: String,
    account: String,
}

impl Display for SlurmAssociation {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "SlurmAssociation {{ user: {}, account: {} }}",
            self.user(),
            self.account()
        )
    }
}

impl SlurmAssociation {
    pub fn from_mapping(mapping: &UserMapping) -> Result<Self, Error> {
        Ok(SlurmAssociation {
            user: clean_user_name(mapping.local_user())?,
            account: clean_account_name(mapping.local_group())?,
        })
    }

    pub fn construct(value: &serde_json::Value) -> Result<Self, Error> {
        let user = match value.get("user") {
            Some(user) => user,
            None => {
                tracing::warn!("Could not get user from association: {:?}", value);
                return Err(Error::Call(
                    "Could not get user from association".to_string(),
                ));
            }
        };

        let user = match user.as_str() {
            Some(user) => user,
            None => {
                tracing::warn!("Could not get user as string from association: {:?}", user);
                return Err(Error::Call(
                    "Could not get user as string from association".to_string(),
                ));
            }
        };

        let account = match value.get("account") {
            Some(account) => account,
            None => {
                tracing::warn!("Could not get account from association: {:?}", value);
                return Err(Error::Call(
                    "Could not get account from association".to_string(),
                ));
            }
        };

        let account = match account.as_str() {
            Some(account) => account,
            None => {
                tracing::warn!(
                    "Could not get account as string from association: {:?}",
                    account
                );
                return Err(Error::Call(
                    "Could not get account as string from association".to_string(),
                ));
            }
        };

        Ok(SlurmAssociation {
            user: user.to_string(),
            account: clean_account_name(account)?,
        })
    }

    pub fn user(&self) -> &str {
        &self.user
    }

    pub fn account(&self) -> &str {
        &self.account
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SlurmUser {
    name: String,
    default_account: Option<String>,
    associations: Vec<SlurmAssociation>,
}

impl Display for SlurmUser {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "SlurmUser {{ name: {}, default: {}, associations: [{}] }}",
            self.name(),
            self.default_account()
                .as_ref()
                .unwrap_or(&"None".to_string()),
            self.associations()
                .iter()
                .map(|a| a.to_string())
                .collect::<Vec<String>>()
                .join(", ")
        )
    }
}

impl SlurmUser {
    pub fn from_mapping(mapping: &UserMapping) -> Result<Self, Error> {
        Ok(SlurmUser {
            name: mapping.local_user().to_string(),
            default_account: Some(clean_account_name(mapping.local_group())?),
            associations: vec![SlurmAssociation::from_mapping(mapping)?],
        })
    }

    pub fn construct(value: &serde_json::Value) -> Result<Self, Error> {
        let name = match value.get("name") {
            Some(name) => match name.as_str() {
                Some(name) => name.to_string(),
                None => {
                    tracing::warn!("Could not get name as string from user: {:?}", name);
                    return Err(Error::Call(
                        "Could not get name as string from user".to_string(),
                    ));
                }
            },
            None => {
                tracing::warn!("Could not get name from user: {:?}", value);
                return Err(Error::Call("Could not get name from user".to_string()));
            }
        };

        let default_account = match value.get("default") {
            Some(default_account) => match default_account.get("account") {
                Some(default_account) => match default_account.as_str() {
                    Some(default_account) => Some(default_account.to_string()),
                    None => {
                        tracing::warn!(
                            "Could not get default_account as string from user: {:?}",
                            default_account
                        );
                        None
                    }
                },
                None => {
                    tracing::warn!(
                        "Could not get default_account as string from user: {:?}",
                        default_account
                    );
                    return Err(Error::Call(
                        "Could not get default_account as string from user".to_string(),
                    ));
                }
            },
            None => None,
        };

        let associations = match value.get("associations") {
            Some(associations) => match associations.as_array() {
                Some(associations) => {
                    let mut slurm_associations: Vec<SlurmAssociation> = Vec::new();

                    for association in associations {
                        slurm_associations.push(SlurmAssociation::construct(association)?);
                    }

                    slurm_associations
                }
                None => {
                    tracing::warn!("Associations is not an array: {:?}", associations);
                    return Err(Error::Call("Associations is not an array".to_string()));
                }
            },
            None => Vec::new(),
        };

        Ok(SlurmUser {
            name,
            default_account,
            associations,
        })
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn default_account(&self) -> &Option<String> {
        &self.default_account
    }

    pub fn associations(&self) -> &Vec<SlurmAssociation> {
        &self.associations
    }
}

pub async fn connect(
    server: &str,
    user: &str,
    token_command: &str,
    token_lifespan: u32,
) -> Result<(), Error> {
    // make sure that the token lifespan is at least 10 seconds
    let token_lifespan = match token_lifespan < 10 {
        true => 10,
        false => token_lifespan,
    };

    // overwrite the global FreeIPA client with a new one
    let mut auth = auth().await?;

    auth.server = server.to_string();
    auth.user = user.to_string();
    auth.token_command = token_command.to_string();
    auth.jwt = Secret::new("".to_string());
    auth.token_lifespan = token_lifespan;
    auth.num_reconnects = 0;

    const MAX_RECONNECTS: u32 = 3;
    const RECONNECT_WAIT: u64 = 100;

    loop {
        match login(
            &auth.server,
            &auth.user,
            &auth.token_command,
            auth.token_lifespan,
        )
        .await
        {
            Ok(session) => {
                auth.jwt = session.jwt;
                auth.jwt_creation_time = session.start_time;
                auth.version = session.version;
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

pub async fn add_user(user: &UserMapping) -> Result<(), Error> {
    let user: SlurmUser = get_user_create_if_not_exists(user).await?;

    tracing::info!("Added user: {}", user);

    Ok(())
}

pub async fn add_project(project: &ProjectMapping) -> Result<(), Error> {
    let account = SlurmAccount::from_mapping(project)?;

    let account = get_account_create_if_not_exists(&account).await?;

    tracing::info!("Added account: {}", account);

    Ok(())
}
