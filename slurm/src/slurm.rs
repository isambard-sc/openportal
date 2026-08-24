// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use anyhow::Context;
use anyhow::Result;
use chrono::{TimeZone, Utc};
use greatwestern::grammar::{DateRange, ProjectMapping, UserMapping};
use greatwestern::usagereport::{ProjectUsageReport, Usage};
use once_cell::sync::Lazy;
use rand::seq::IteratorRandom;
use rand::SeedableRng;
use reqwest::{Client, Url};
use secrecy::{ExposeSecret, SecretString};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::fmt::Display;
use std::sync::Arc;
use std::time::Duration;
use templemeads::job::assert_not_expired;
use templemeads::Error;
use tokio::sync::Mutex;

use crate::cache;
use crate::sacctmgr;

#[derive(Debug, Clone)]
struct SlurmServer {
    server: String,
    token_command: String,
    token_lifespan: u32,
    user: String,
    jwt: SecretString,
    jwt_creation_time: u64,
    version: String,
    num_failed_reconnects: u32,
    last_failed_reconnect: Option<chrono::DateTime<Utc>>,
}

impl SlurmServer {
    fn new(server: &str, user: &str, token_command: &str, token_lifespan: u32) -> Self {
        SlurmServer {
            server: server.to_string(),
            token_command: token_command.to_string(),
            token_lifespan,
            user: user.to_string(),
            jwt: SecretString::default(),
            jwt_creation_time: 0,
            version: String::new(),
            num_failed_reconnects: 0,
            last_failed_reconnect: None,
        }
    }

    fn is_logged_in(&self) -> bool {
        !self.jwt.expose_secret().is_empty() && !self.token_expired().unwrap_or(true)
    }

    fn token_expired(&self) -> Result<bool, Error> {
        if self.jwt.expose_secret().is_empty() {
            return Ok(true);
        }

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .context("Could not get current time")?;

        // we give ourselves a 10 second margin of error
        Ok(10 + now.as_secs() - self.jwt_creation_time > self.token_lifespan as u64)
    }

    fn set_login_failed(&mut self) {
        self.num_failed_reconnects += 1;
        self.last_failed_reconnect = Some(Utc::now());
        self.jwt = SecretString::default();
    }

    fn set_login_success(&mut self, session: SlurmSession) {
        self.jwt = session.jwt;
        self.jwt_creation_time = session.start_time;
        self.version = session.version;
        self.num_failed_reconnects = 0;
        self.last_failed_reconnect = None;
    }

    fn should_backoff(&self) -> bool {
        if self.num_failed_reconnects < 3 {
            return false;
        }

        if let Some(last_failed) = self.last_failed_reconnect {
            let backoff_duration =
                chrono::Duration::seconds(20 * self.num_failed_reconnects as i64);
            Utc::now() < last_failed + backoff_duration
        } else {
            false
        }
    }
}

#[derive(Debug)]
struct LockedSlurmServer {
    server: tokio::sync::OwnedMutexGuard<SlurmServer>,
}

impl LockedSlurmServer {
    fn server(&self) -> &str {
        &self.server.server
    }

    fn user(&self) -> &str {
        &self.server.user
    }

    fn version(&self) -> &str {
        &self.server.version
    }

    fn jwt(&self) -> &SecretString {
        &self.server.jwt
    }

    fn set_login_failed(&mut self) {
        self.server.set_login_failed();
    }
}

static SLURM_SERVERS: Lazy<Mutex<Vec<Arc<Mutex<SlurmServer>>>>> =
    Lazy::new(|| Mutex::new(Vec::new()));

struct SlurmSession {
    jwt: SecretString,
    version: String,
    start_time: u64,
}

/// Split a Slurm `openapi.json` `info.version` string (`"dbvX.Y.Z"`, sometimes
/// with a trailing `"&…"`) into the bare version string and its numeric
/// components.
///
/// Extracted from `login` so it can be tested without a server: the string is
/// wholly attacker-controlled if `slurm-server` is `http://` or the
/// `slurmrestd` is compromised, and it used to be indexed unconditionally at
/// element 2. See docs/specifications/security-review-2.md (findings R1/R27).
fn parse_api_version(version: &str) -> Result<(String, Vec<u32>), Error> {
    let version = version
        .split('v')
        .nth(1)
        .context("Could not split version")?;

    // sometimes there is an additional '&something' afterwards - remove it
    let version = version
        .split('&')
        .next()
        .context("Could not split version")?
        .to_string();

    let version_numbers: Vec<u32> = version
        .split('.')
        .map(|x| x.parse::<u32>())
        .collect::<Result<Vec<u32>, _>>()
        .context("Could not parse version numbers")?;

    if version_numbers.is_empty() {
        return Err(Error::Login(format!(
            "Slurm reported an empty version string: '{}'",
            version
        )));
    }

    Ok((version, version_numbers))
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
    expires: &chrono::DateTime<Utc>,
) -> Result<SlurmSession, Error> {
    assert_not_expired(expires)?;

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

    // Do not log the token command itself - some deployments embed a
    // credential in it, which would then land in the logs (finding F15).
    tracing::info!("Getting JWT token from the configured token command");

    // parse 'token_command' into an executable plus arguments
    let token_command = shlex::split(&token_command).context("Could not parse token command")?;

    let token_exe = token_command.first().context("No token command")?;
    let token_args = token_command.get(1..).unwrap_or(&[]);

    // get the current datetime in seconds since the epoch - we will use this
    // to check the token expiry
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .context("Could not get current time")?;

    assert_not_expired(expires)?;

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
    //
    // The raw output must never appear in the error: it is the one place
    // credential-bearing output could escape to the requesting peer, since a `Result`
    // from here is returned up the Job chain. F15 fixed the *logging* of the token
    // command; this is the argv/stdout counterpart. See
    // `docs/specifications/security-review-2.md` (finding R33).
    let jwt = jwt
        .split_whitespace()
        .find(|x| x.contains("="))
        .context(
            "Could not find a '<name>=<token>' field in the token command's output. \
             The output is not reported here because it may contain a credential - \
             run the configured token command by hand to see it.",
        )?
        .split('=')
        .nth(1)
        .context(
            "Could not extract the token from the token command's output (the output \
             is not reported here because it may contain a credential).",
        )?
        .to_string();

    assert_not_expired(expires)?;

    // create a client
    let client = Client::builder()
        .timeout(Duration::from_secs(60))
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
        .to_string();

    let (version, mut version_numbers) = parse_api_version(&version)?;

    let mut working_version = None;

    // the Slurm API supports normally 3 versions - this has reported the
    // lowest version - see how many higher versions we can use
    tracing::info!("Auto detecting maximum version of the Slurm API...");
    loop {
        assert_not_expired(expires)?;

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

        // The patch component is supplied by the Slurm server (it is parsed
        // out of `openapi.json`'s `info.version`), so a two-component or
        // otherwise unexpected version string must not be able to abort this
        // process - see docs/specifications/security-review-2.md (findings
        // R1/R27).
        match version_numbers.get_mut(2).and_then(|patch| {
            // `checked_add` rather than `+=`: the patch component comes
            // straight from the server, and release builds now check overflow,
            // so a reported patch of u32::MAX would otherwise abort here.
            *patch = patch.checked_add(1)?;
            Some(())
        }) {
            Some(()) => {}
            None => {
                tracing::warn!(
                    "Slurm reported version '{}', which does not have the \
                     expected major.minor.patch form - not probing for a \
                     higher version.",
                    version
                );
                break;
            }
        }
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

    assert_not_expired(expires)?;

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
        let Some(default_cluster) = clusters.first() else {
            return Err(Error::Login(
                "Slurm reported no clusters at all - cannot pick a default".to_string(),
            ));
        };

        tracing::info!(
            "Using the first cluster available by default: {}",
            default_cluster
        );
        cache::set_cluster(default_cluster).await?;
    }

    Ok(SlurmSession {
        jwt: jwt.into(),
        version: version.to_string(),
        start_time: now.as_secs(),
    })
}

async fn get_connected_server(expires: &chrono::DateTime<Utc>) -> Result<LockedSlurmServer, Error> {
    // get a copy of the servers, so that we don't hold the lock while we
    // try to connect
    assert_not_expired(expires)?;

    let slurm_servers = SLURM_SERVERS.lock().await.clone();

    if slurm_servers.is_empty() {
        return Err(Error::Call(
            "No Slurm servers have been initialised".to_string(),
        ));
    }

    let mut rng = rand::rngs::StdRng::from_os_rng();

    loop {
        let mut should_all_backoff: bool = true;

        // randomise the order of the servers for each loop
        for server in slurm_servers
            .iter()
            .choose_multiple(&mut rng, slurm_servers.len())
        {
            assert_not_expired(expires)?;

            match server.clone().try_lock_owned() {
                Ok(mut server) => {
                    if server.is_logged_in() {
                        tracing::debug!("Already logged in to Slurm server: {}", server.server);
                        return Ok(LockedSlurmServer { server });
                    }

                    if server.should_backoff() {
                        tracing::warn!(
                            "Backing off from trying to login to Slurm server: {}",
                            server.server
                        );
                        continue;
                    }

                    should_all_backoff = false;

                    tracing::info!("Logging in to Slurm server: {}", server.server);
                    match login(
                        &server.server,
                        &server.user,
                        &server.token_command,
                        server.token_lifespan,
                        expires,
                    )
                    .await
                    {
                        Ok(session) => {
                            // update the server with the new session
                            tracing::info!("Login successful to Slurm server: {}", server.server);
                            server.set_login_success(session);
                            return Ok(LockedSlurmServer { server });
                        }
                        Err(e) => {
                            tracing::error!(
                                "Could not login to Slurm server: {}. Error: {}",
                                server.server,
                                e
                            );
                            server.set_login_failed();

                            // release the lock and try the next server
                        }
                    }
                }
                Err(_) => {
                    // could not get the lock - this implies the server is in use
                    should_all_backoff = false;
                }
            }
        }

        if should_all_backoff {
            tracing::error!(
                "All Slurm servers are backing off because of repeated login failures."
            );
            return Err(Error::Call(
                "All Slurm servers are backing off because of repeated login failures.".to_string(),
            ));
        }

        // wait a bit before trying again
        assert_not_expired(expires)?;
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

///
/// Call a get URL on the slurmrestd server described in 'auth'.
///
async fn call_get(
    backend: &str,
    function: &str,
    query_params: &Vec<(&str, &str)>,
    expires: &chrono::DateTime<Utc>,
) -> Result<serde_json::Value, Error> {
    // get a connected server
    tracing::debug!("Getting a connected server...");
    let start_time = Utc::now();
    let mut lock = get_connected_server(expires).await?;
    tracing::debug!(
        "Connected server obtained! Took {} ms",
        (Utc::now() - start_time).num_milliseconds()
    );

    // how much time is left before we expire?
    let time_left = expires.signed_duration_since(Utc::now()).num_seconds();

    if time_left < 5 {
        return Err(Error::Call(
            "Not enough time left to call Slurm server".to_string(),
        ));
    }

    tracing::debug!(
        "Calling Slurm function: {} - we have {} seconds left before we expire",
        function,
        time_left
    );

    let url = match Url::parse_with_params(
        &format!(
            "{}/{}/v{}/{}",
            lock.server(),
            backend,
            lock.version(),
            function
        ),
        query_params,
    ) {
        Ok(url) => url,
        Err(e) => {
            tracing::error!("Could not parse URL: {}", e);
            return Err(Error::Call("Could not parse URL".to_string()));
        }
    };

    tracing::debug!("Calling function {}", url);

    let client = Client::builder()
        .timeout(Duration::from_secs(time_left.min(60) as u64))
        .build()
        .context("Could not build client")?;

    let mut result = client
        .get(url.clone())
        .header("Referer", format!("{}/ipa", lock.server()))
        .header("Content-Type", "application/json")
        .header("Accept", "application/json")
        .header("X-SLURM-USER-NAME", lock.user())
        .header("X-SLURM-USER-TOKEN", lock.jwt().expose_secret().to_string())
        .send()
        .await
        .with_context(|| format!("Could not call function: {}", url))?;

    // write a warning if this took a long time
    if (Utc::now() - start_time).num_seconds() > 5 {
        tracing::warn!(
            "Slurm call for server {} to function {} took {} seconds",
            lock.server(),
            function,
            (Utc::now() - start_time).num_seconds()
        );
    } else {
        tracing::debug!(
            "Slurm call for server {} to function {} took {} ms",
            lock.server(),
            function,
            (Utc::now() - start_time).num_milliseconds()
        );
    }

    // if this is an authorisation error, try to reconnect
    while result.status().as_u16() == 401 {
        tracing::warn!("Login error: 401 - authorisation failed.");
        lock.set_login_failed();

        // try to get another lock
        drop(lock);

        assert_not_expired(expires)?;

        tracing::error!("Authorisation (401) error. Reconnecting.");
        lock = get_connected_server(expires).await?;

        if Utc::now().signed_duration_since(start_time).num_seconds() > 10 {
            tracing::info!(
                "Call to server {} for function {} is still running... {} seconds elapsed so far.",
                lock.server(),
                function,
                (Utc::now() - start_time).num_seconds()
            );
        }

        let time_left = expires.signed_duration_since(Utc::now()).num_seconds();

        if time_left < 5 {
            return Err(Error::Call(
                "Not enough time left to call Slurm server".to_string(),
            ));
        }

        let client = Client::builder()
            .timeout(Duration::from_secs(time_left.min(60) as u64))
            .build()
            .context("Could not build client")?;

        // retry the call
        result = client
            .get(url.clone())
            .header("Referer", format!("{}/ipa", lock.server()))
            .header("Content-Type", "application/json")
            .header("Accept", "application/json")
            .header("X-SLURM-USER-NAME", lock.user())
            .header("X-SLURM-USER-TOKEN", lock.jwt().expose_secret().to_string())
            .send()
            .await
            .with_context(|| format!("Could not call function: {}", url))?;

        if Utc::now().signed_duration_since(start_time).num_seconds() > 10 {
            tracing::error!(
                "Call to server {} for function {} has completed... {} seconds elapsed so far.",
                lock.server(),
                function,
                (Utc::now() - start_time).num_seconds()
            );
        }
    }

    if result.status().as_u16() == 500 {
        tracing::error!(
            "500 error - slurmrestd error when calling {} as user {}.",
            url,
            lock.user()
        );

        match result.json::<serde_json::Value>().await {
            Ok(json) => tracing::error!("Server response: {}", json),
            Err(_) => tracing::error!("Could not decode the server's response."),
        };

        return Err(Error::Call(format!(
            "500 error - slurmrestd error when calling {} as user {}.",
            url,
            lock.user()
        )));
    }

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
    expires: &chrono::DateTime<Utc>,
) -> Result<(), Error> {
    // get a connected server
    tracing::debug!("Getting a connected server...");
    let start_time = Utc::now();
    let mut lock = get_connected_server(expires).await?;
    tracing::debug!(
        "Connected server obtained! Took {} ms",
        (Utc::now() - start_time).num_milliseconds()
    );

    // how much time is left before we expire?
    let time_left = expires.signed_duration_since(Utc::now()).num_seconds();

    if time_left < 5 {
        return Err(Error::Call(
            "Not enough time left to call Slurm server".to_string(),
        ));
    }

    tracing::debug!(
        "Calling Slurm function: {} - we have {} seconds left before we expire",
        function,
        time_left
    );

    let url = format!(
        "{}/{}/v{}/{}",
        lock.server(),
        backend,
        lock.version(),
        function
    );

    tracing::debug!("Calling function {} with payload: {:?}", url, payload);

    let client = Client::builder()
        .timeout(Duration::from_secs(time_left.min(60) as u64))
        .build()
        .context("Could not build client")?;

    let mut result = client
        .post(&url)
        .header("Referer", format!("{}/ipa", lock.server()))
        .header("Content-Type", "application/json")
        .header("Accept", "application/json")
        .header("X-SLURM-USER-NAME", lock.user())
        .header("X-SLURM-USER-TOKEN", lock.jwt().expose_secret().to_string())
        .json(&payload)
        .send()
        .await
        .with_context(|| format!("Could not call function: {}", url))?;

    // write a warning if this took a long time
    if (Utc::now() - start_time).num_seconds() > 5 {
        tracing::warn!(
            "Slurm call for server {} to function {} took {} seconds",
            lock.server(),
            function,
            (Utc::now() - start_time).num_seconds()
        );
    } else {
        tracing::debug!(
            "Slurm call for server {} to function {} took {} ms",
            lock.server(),
            function,
            (Utc::now() - start_time).num_milliseconds()
        );
    }

    // if this is an authorisation error, try to reconnect
    while result.status().as_u16() == 401 {
        tracing::warn!("Login error: 401 - authorisation failed.");
        lock.set_login_failed();

        // try to get another lock
        drop(lock);

        assert_not_expired(expires)?;

        tracing::error!("Authorisation (401) error. Reconnecting.");
        lock = get_connected_server(expires).await?;

        if Utc::now().signed_duration_since(start_time).num_seconds() > 10 {
            tracing::info!(
                "Call to server {} for function {} is still running... {} seconds elapsed so far.",
                lock.server(),
                function,
                (Utc::now() - start_time).num_seconds()
            );
        }

        let time_left = expires.signed_duration_since(Utc::now()).num_seconds();

        if time_left < 5 {
            return Err(Error::Call(
                "Not enough time left to call Slurm server".to_string(),
            ));
        }

        let client = Client::builder()
            .timeout(Duration::from_secs(time_left.min(60) as u64))
            .build()
            .context("Could not build client")?;

        // retry the call
        result = client
            .post(&url)
            .header("Referer", format!("{}/ipa", lock.server()))
            .header("Content-Type", "application/json")
            .header("Accept", "application/json")
            .header("X-SLURM-USER-NAME", lock.user())
            .header("X-SLURM-USER-TOKEN", lock.jwt().expose_secret().to_string())
            .json(&payload)
            .send()
            .await
            .with_context(|| format!("Could not call function: {}", url))?;

        if Utc::now().signed_duration_since(start_time).num_seconds() > 10 {
            tracing::error!(
                "Call to server {} for function {} has completed... {} seconds elapsed so far.",
                lock.server(),
                function,
                (Utc::now() - start_time).num_seconds()
            );
        }
    }

    if result.status().as_u16() == 500 {
        tracing::error!(
            "500 error - slurmrestd error when calling {} with payload {} as user {}.",
            url,
            payload,
            lock.user()
        );

        match result.json::<serde_json::Value>().await {
            Ok(json) => tracing::error!("Server response: {}", json),
            Err(_) => tracing::error!("Could not decode the server's response."),
        };

        return Err(Error::Call(format!(
            "500 error - slurmrestd error when calling {} with payload {} as user {}.",
            url,
            payload,
            lock.user()
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

async fn force_add_slurm_account(
    account: &SlurmAccount,
    expires: &chrono::DateTime<Utc>,
) -> Result<SlurmAccount, Error> {
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

    assert_not_expired(expires)?;

    let cluster = cache::get_cluster().await?;
    let parent_account = cache::get_parent_account().await?;

    let payload = serde_json::json!({
        "accounts": [
            {
                "name": account.name,
                "description": account.description,
                "organization": account.organization,
                "cluster": cluster,
                "parent": parent_account
            }
        ]
    });

    call_post("slurmdb", "accounts", &payload, expires).await?;

    Ok(account.clone())
}

async fn get_account_from_slurm(
    account: &str,
    expires: &chrono::DateTime<Utc>,
) -> Result<Option<SlurmAccount>, Error> {
    let account = clean_account_name(account)?;

    let response = match call_get(
        "slurmdb",
        &format!("account/{}", encode_path_segment(&account)),
        &vec![("with_assocs", "true")],
        expires,
    )
    .await
    {
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

    let account = match SlurmAccount::construct(account) {
        Ok(account) => account,
        Err(e) => {
            tracing::warn!("Could not construct account from response: {}", e);
            return Ok(None);
        }
    };

    cache::add_account(&account).await?;

    let cluster = cache::get_cluster().await?;

    match account.in_cluster(&cluster) {
        true => Ok(Some(account)),
        false => {
            tracing::warn!(
                "Account {} is not in cluster {} - ignoring",
                account.name(),
                cluster
            );
            Ok(None)
        }
    }
}

async fn get_account(
    account: &str,
    expires: &chrono::DateTime<Utc>,
) -> Result<Option<SlurmAccount>, Error> {
    // need to GET /slurm/vX.Y.Z/accounts/{account.name}
    // and return the account if it exists
    let cached_account = cache::get_account(account).await?;

    if let Some(cached_account) = cached_account {
        // double-check that the account actually exists...
        let existing_account = match get_account_from_slurm(cached_account.name(), expires).await {
            Ok(account) => account,
            Err(e) => {
                tracing::warn!("Could not get account {}: {}", cached_account.name(), e);
                cache::remove_account(cached_account.name()).await?;
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

                // only this account is known to be stale - see cache::remove_account
                cache::remove_account(cached_account.name()).await?;

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
            cache::remove_account(cached_account.name()).await?;
            return Ok(None);
        }
    }

    // see if we can read the account from slurm
    let account = match get_account_from_slurm(account, expires).await {
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

async fn get_account_create_if_not_exists(
    account: &SlurmAccount,
    expires: &chrono::DateTime<Utc>,
) -> Result<SlurmAccount, Error> {
    let existing_account = get_account(account.name(), expires).await?;

    let cluster = cache::get_cluster().await?;

    if let Some(existing_account) = existing_account {
        if existing_account.in_cluster(&cluster) {
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

            tracing::debug!("Using existing slurm account {}", existing_account);
            return Ok(existing_account);
        }
    }

    // it doesn't, so create it
    tracing::info!("Creating new slurm account: {}", account.name());
    let account = force_add_slurm_account(account, expires).await?;

    // get the account as created
    match get_account(account.name(), expires).await {
        Ok(Some(account)) => Ok(account),
        Ok(None) => {
            tracing::error!("Could not get account {}", account.name());
            Err(Error::NotFound(account.name().to_string()))
        }
        Err(e) => {
            tracing::error!("Could not get account {}: {}", account.name(), e);
            Err(e)
        }
    }
}

async fn get_user_from_slurm(
    user: &str,
    expires: &chrono::DateTime<Utc>,
) -> Result<Option<SlurmUser>, Error> {
    let user = clean_user_name(user)?;

    let query_params = vec![("with_assocs", "true"), ("default_account", "true")];

    let response = match call_get(
        "slurmdb",
        &format!("user/{}", encode_path_segment(&user)),
        &query_params,
        expires,
    )
    .await
    {
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

async fn get_user(user: &str, expires: &chrono::DateTime<Utc>) -> Result<Option<SlurmUser>, Error> {
    let cached_user = cache::get_user(user).await?;

    if let Some(cached_user) = cached_user {
        // double-check that the user actually exists...
        let existing_user = match get_user_from_slurm(cached_user.name(), expires).await {
            Ok(user) => user,
            Err(e) => {
                tracing::warn!("Could not get user {}: {}", cached_user.name(), e);
                cache::remove_user(cached_user.name()).await?;
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

                // only this user is known to be stale - see cache::remove_user
                cache::remove_user(cached_user.name()).await?;

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
            cache::remove_user(cached_user.name()).await?;
            return Ok(None);
        }
    }

    // see if we can read the user from slurm
    let user = match get_user_from_slurm(user, expires).await {
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

async fn add_account_association(
    account: &SlurmAccount,
    expires: &chrono::DateTime<Utc>,
) -> Result<(), Error> {
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

    assert_not_expired(expires)?;

    // get the cluster name from the cache
    let cluster = cache::get_cluster().await?;

    // get the parent account name from the cache
    let parent_account = cache::get_parent_account().await?;

    // add the association condition to the account
    let payload = serde_json::json!({
        "association_condition": {
            "accounts": [account.name],
            "clusters": [cluster],
            "parent": [parent_account],
            "association": {
                "defaultqos": "normal",
                "comment": format!("Association added by OpenPortal for account {}", account.name)
            }
        }
    });

    call_post("slurmdb", "accounts_association", &payload, expires).await?;

    Ok(())
}

async fn add_user_association(
    user: &SlurmUser,
    account: &SlurmAccount,
    make_default: bool,
    expires: &chrono::DateTime<Utc>,
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
    let cluster = cache::get_cluster().await?;

    // first, add the association if it doesn't exist
    if !user
        .associations()
        .iter()
        .any(|a| a.account() == account.name() && a.cluster() == cluster)
    {
        // make sure that we have this association on the account
        add_account_association(account, expires).await?;

        // now add the association to the user
        let payload = serde_json::json!({
            "associations": [
                {
                    "user": user.name,
                    "account": account.name,
                    "comment": format!("Association added by OpenPortal between user {} and account {}",
                                       user.name, account.name),
                    "cluster": cluster,
                    "is_default": true
                }
            ]
        });

        call_post("slurmdb", "associations", &payload, expires).await?;

        // update the user
        user = match get_user_from_slurm(user.name(), expires).await? {
            Some(user) => user,
            None => {
                return Err(Error::Call(format!(
                    "Could not get user that just had its associations updated! '{}'",
                    user.name()
                )))
            }
        };

        user_changed = true;

        tracing::debug!("Updated user: {}", user);
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

        call_post("slurmdb", "users", &payload, expires).await?;

        // update the user
        user = match get_user_from_slurm(user.name(), expires).await? {
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
        tracing::debug!("Using existing user: {}", user);
    }

    Ok(user)
}

async fn get_user_create_if_not_exists(
    user: &UserMapping,
    expires: &chrono::DateTime<Utc>,
) -> Result<SlurmUser, Error> {
    // first, make sure that the account exists
    let slurm_account = get_account_create_if_not_exists(
        &SlurmAccount::from_mapping(&user.clone().into())?,
        expires,
    )
    .await?;

    let slurm_user = get_user(user.local_user().unix()?, expires).await?;
    let cluster = cache::get_cluster().await?;

    if let Some(slurm_user) = slurm_user {
        // the user exists - check that the account is associated with the user
        if *slurm_user.default_account() == Some(slurm_account.name().to_string())
            && slurm_user
                .associations()
                .iter()
                .any(|a| a.account() == slurm_account.name() && a.cluster() == cluster)
        {
            tracing::debug!("Using existing user {}", slurm_user);
            return Ok(slurm_user);
        } else {
            tracing::warn!(
                "User {} exists, but is not default associated with the requested account '{}' in cluster {}.",
                user,
                slurm_account,
                cluster
            );
        }
    }

    assert_not_expired(expires)?;

    // first, create the user
    let username = clean_user_name(user.local_user().unix()?)?;

    let payload = serde_json::json!({
        "users": [
            {
                "name": username,
            }
        ]
    });

    call_post("slurmdb", "users", &payload, expires).await?;

    // now load the user from slurm to make sure it exists
    let slurm_user = match get_user(user.local_user().unix()?, expires).await? {
        Some(user) => user,
        None => {
            return Err(Error::Call(format!(
                "Could not get user that was just created! '{}'",
                user.local_user()
            )))
        }
    };

    // now add the association to the account, making it the default
    let slurm_user = add_user_association(&slurm_user, &slurm_account, true, expires).await?;

    let user = SlurmUser::from_mapping(user)?;

    // check we have the user we expected
    if slurm_user != user {
        tracing::warn!("User {} exists, but with different details.", user.name());
        tracing::warn!("Existing: {:?}, new: {:?}", slurm_user, user);
    }

    Ok(slurm_user)
}

///
/// Return the organization that indicates that this user / account is managed
///
pub fn get_managed_organization() -> String {
    "openportal".to_string()
}

pub async fn initialise_servers(
    servers: &[String],
    users: &[String],
    token_commands: &[String],
    token_lifespans: &[u32],
) -> Result<(), Error> {
    let mut slurm_servers = SLURM_SERVERS.lock().await;

    // clear any existing servers
    slurm_servers.clear();

    // make sure all vectors have the same length
    if servers.len() != users.len()
        || servers.len() != token_commands.len()
        || servers.len() != token_lifespans.len()
    {
        return Err(Error::Call(
            "All server configuration vectors must have the same length".to_string(),
        ));
    }

    // now add each server
    for (i, server) in servers.iter().enumerate() {
        let server = server.trim();

        if server.is_empty() {
            continue;
        }

        // The lengths were checked equal above; read via `get` so that a
        // future change to that check cannot turn into a panic - see
        // docs/specifications/security-review-2.md (finding R1).
        let user = users.get(i).map(|u| u.trim()).unwrap_or_default();
        let token_command = token_commands.get(i).map(|c| c.trim()).unwrap_or_default();
        let token_lifespan = token_lifespans.get(i).copied().unwrap_or(10).max(10);

        slurm_servers.push(Arc::new(Mutex::new(SlurmServer::new(
            server,
            user,
            token_command,
            token_lifespan,
        ))));
    }

    Ok(())
}

/// Percent-encode `segment` so it is safe to interpolate into a URL *path*.
///
/// Everything outside the RFC 3986 unreserved set (`A-Za-z0-9-._~`) is encoded,
/// so a name containing `?`, `#`, `/`, `%`, `&` or `=` cannot inject a query
/// parameter, an extra path segment, or a fragment into a slurmrestd request.
/// `Url::parse_with_params` *appends* to whatever query the interpolated string
/// introduces, so a name of `x?with_deleted=true` previously became a real
/// query parameter.
///
/// Mapping targets are now restricted to `[A-Za-z0-9_.-]` upstream
/// (`templemeads::validate::validate_mapping_target`), so nothing reaching here
/// from a peer needs encoding - but account and user names also come back from
/// Slurm itself, and this must not depend on that upstream guard staying
/// exactly as it is. See `docs/specifications/security-review-2.md` (finding
/// R14).
fn encode_path_segment(segment: &str) -> String {
    let mut encoded = String::with_capacity(segment.len());

    for byte in segment.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                encoded.push(byte as char)
            }
            _ => encoded.push_str(&format!("%{:02X}", byte)),
        }
    }

    encoded
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
    limit: Usage,
    clusters: HashSet<String>,
}

impl PartialEq for SlurmAccount {
    fn eq(&self, other: &Self) -> bool {
        self.name == other.name
            && self.organization == other.organization
            && self.description.eq_ignore_ascii_case(&other.description)
            // && self.limit == other.limit // ignore limit for now as it is not set
            && self.clusters == other.clusters
    }
}

impl Display for SlurmAccount {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "SlurmAccount {{ name: {}, description: {}, organization: {}, limit: {}, clusters: {:?} }}",
            self.name(),
            self.description(),
            self.organization(),
            self.limit(),
            self.clusters()
        )
    }
}

impl SlurmAccount {
    pub fn from_mapping(mapping: &ProjectMapping) -> Result<Self, Error> {
        let name = clean_account_name(match mapping.local_group().starts_with("group.") {
            //if it starts with "group.X" then return "X" as this is legacy account
            true => mapping
                .local_group()
                .split('.')
                .nth(1)
                .unwrap_or(mapping.local_group()),
            false => mapping.local_group(),
        })?;

        Ok(SlurmAccount {
            name,
            description: format!("Account for OpenPortal project {}", mapping.project()),
            organization: get_managed_organization(),
            limit: Usage::default(),
            clusters: HashSet::new(),
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

        let associations = match result.get("associations") {
            Some(associations) => associations.clone(),
            None => {
                tracing::warn!("Could not get associations from account: {:?}", result);
                serde_json::Value::Array(Vec::new())
            }
        };

        let associations = match associations.as_array() {
            Some(associations) => associations.clone(),
            None => {
                tracing::warn!("Associations is not an array: {:?}", associations);
                Vec::new()
            }
        };

        let mut clusters = HashSet::new();

        for association in associations {
            let cluster = match association.get("cluster") {
                Some(cluster) => cluster,
                None => {
                    tracing::warn!("Could not get cluster from association: {:?}", association);
                    continue;
                }
            };

            let cluster = match cluster.as_str() {
                Some(cluster) => cluster,
                None => {
                    tracing::warn!(
                        "Could not get cluster as string from association: {:?}",
                        cluster
                    );
                    continue;
                }
            };

            clusters.insert(cluster.to_string());
        }

        Ok(SlurmAccount {
            name: clean_account_name(name)?,
            description: description.to_string(),
            organization: organization.to_string(),
            limit: Usage::default(),
            clusters,
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

    pub fn limit(&self) -> &Usage {
        &self.limit
    }

    pub fn set_limit(&mut self, limit: &Usage) {
        self.limit = *limit;
    }

    pub fn clusters(&self) -> &HashSet<String> {
        &self.clusters
    }

    pub fn in_cluster(&self, cluster: &str) -> bool {
        self.clusters.contains(cluster)
    }

    pub fn is_managed(&self) -> bool {
        self.organization == get_managed_organization()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SlurmLimit {
    account: String,
    cluster: String,
    cpu_limit: Option<Usage>,
    gpu_limit: Option<Usage>,
    mem_limit: Option<Usage>,
    billing_limit: Option<Usage>,
}

impl Display for SlurmLimit {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "SlurmLimit {{ account: {}, cluster: {}, cpu: {:?}, gpu: {:?}, mem: {:?}, billing: {:?} }}",
            self.account(),
            self.cluster(),
            self.cpu_limit(),
            self.gpu_limit(),
            self.mem_limit(),
            self.billing_limit()
        )
    }
}

impl SlurmLimit {
    pub fn construct(result: &serde_json::Value) -> Result<Self, Error> {
        let account = match result.get("account") {
            Some(account) => account,
            None => {
                tracing::warn!("Could not get account from limit: {:?}", result);
                return Err(Error::Call("Could not get account from limit".to_string()));
            }
        };

        let account = match account.as_str() {
            Some(account) => account,
            None => {
                tracing::warn!("Could not get account as string from limit: {:?}", account);
                return Err(Error::Call(
                    "Could not get account as string from limit".to_string(),
                ));
            }
        };

        let cluster = match result.get("cluster") {
            Some(cluster) => cluster,
            None => {
                tracing::warn!("Could not get cluster from limit: {:?}", result);
                return Err(Error::Call("Could not get cluster from limit".to_string()));
            }
        };

        let cluster = match cluster.as_str() {
            Some(cluster) => cluster,
            None => {
                tracing::warn!("Could not get cluster as string from limit: {:?}", cluster);
                return Err(Error::Call(
                    "Could not get cluster as string from limit".to_string(),
                ));
            }
        };

        let limits: &Vec<serde_json::Value> = match result.get("max") {
            Some(max) => match max.get("tres") {
                Some(tres) => match tres.get("group") {
                    Some(group) => match group.get("minutes") {
                        Some(minutes) => match minutes.as_array() {
                            Some(limits) => limits,
                            None => {
                                tracing::warn!("Limits is not an array: {:?}", minutes);
                                return Err(Error::Call("Limits is not an array".to_string()));
                            }
                        },
                        None => {
                            tracing::warn!("Could not get minutes from group: {:?}", group);
                            return Err(Error::Call(
                                "Could not get minutes from group".to_string(),
                            ));
                        }
                    },
                    None => {
                        tracing::warn!("Could not get group from tres: {:?}", tres);
                        return Err(Error::Call("Could not get group from tres".to_string()));
                    }
                },
                None => {
                    tracing::warn!("Could not get tres from max: {:?}", max);
                    return Err(Error::Call("Could not get tres from max".to_string()));
                }
            },
            None => {
                tracing::warn!("Could not get max from limit: {:?}", result);
                return Err(Error::Call("Could not get max from limit".to_string()));
            }
        };

        let mut cpu_limit = None;
        let mut gpu_limit = None;
        let mut mem_limit = None;
        let mut billing_limit = None;

        for limit in limits {
            let typ = match limit.get("type") {
                Some(typ) => match typ.as_str() {
                    Some(typ) => typ,
                    None => {
                        tracing::warn!("Could not get type as string from limit: {:?}", typ);
                        continue;
                    }
                },
                None => {
                    tracing::warn!("Could not get type from limit: {:?}", limit);
                    continue;
                }
            };

            let name = match limit.get("name") {
                Some(name) => match name.as_str() {
                    Some(name) => name,
                    None => {
                        tracing::warn!("Could not get name as string from limit: {:?}", limit);
                        return Err(Error::Call(
                            "Could not get name as string from limit".to_string(),
                        ));
                    }
                },
                None => {
                    tracing::warn!("Could not get name from limit: {:?}", limit);
                    continue;
                }
            };

            let count = match limit.get("count") {
                Some(count) => match count.as_u64() {
                    Some(count) => count,
                    None => {
                        tracing::warn!("Could not get count as u64 from limit: {:?}", count);
                        continue;
                    }
                },
                None => {
                    tracing::warn!("Could not get count from limit: {:?}", limit);
                    continue;
                }
            };

            // extract the usages - note that these are returned in minutes,
            // so we need to convert them to seconds
            match typ.to_ascii_lowercase().as_str() {
                "cpu" => cpu_limit = Some(Usage::new(count * 60)),
                "mem" => mem_limit = Some(Usage::new(count * 60)),
                "gres" => match name {
                    "gpu" => gpu_limit = Some(Usage::new(count * 60)),
                    _ => {
                        tracing::warn!("Unknown gres name: {}", name);
                        continue;
                    }
                },
                "billing" => billing_limit = Some(Usage::new(count * 60)),
                _ => {
                    tracing::warn!("Unknown limit type: {}", typ);
                    continue;
                }
            }
        }

        Ok(SlurmLimit {
            account: clean_account_name(account)?,
            cluster: cluster.to_string(),
            cpu_limit,
            gpu_limit,
            mem_limit,
            billing_limit,
        })
    }

    pub fn account(&self) -> &str {
        &self.account
    }

    pub fn cluster(&self) -> &str {
        &self.cluster
    }

    pub fn cpu_limit(&self) -> Option<Usage> {
        self.cpu_limit
    }

    pub fn gpu_limit(&self) -> Option<Usage> {
        self.gpu_limit
    }

    pub fn mem_limit(&self) -> Option<Usage> {
        self.mem_limit
    }

    pub fn billing_limit(&self) -> Option<Usage> {
        self.billing_limit
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SlurmAssociation {
    user: String,
    account: String,
    cluster: String,
}

impl Display for SlurmAssociation {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "SlurmAssociation {{ user: {}, account: {}, cluster: {} }}",
            self.user(),
            self.account(),
            self.cluster()
        )
    }
}

impl SlurmAssociation {
    pub fn from_mapping(mapping: &UserMapping) -> Result<Self, Error> {
        let account = clean_account_name(match mapping.local_group().starts_with("group.") {
            //if it starts with "group.X" then return "X" as this is legacy account
            true => mapping
                .local_group()
                .split('.')
                .nth(1)
                .unwrap_or(mapping.local_group()),
            false => mapping.local_group(),
        })?;

        Ok(SlurmAssociation {
            user: clean_user_name(mapping.local_user().unix()?)?,
            account,
            cluster: "".to_string(),
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

        let cluster = match value.get("cluster") {
            Some(cluster) => cluster,
            None => {
                tracing::warn!("Could not get cluster from association: {:?}", value);
                return Err(Error::Call(
                    "Could not get cluster from association".to_string(),
                ));
            }
        };

        let cluster = match cluster.as_str() {
            Some(cluster) => cluster,
            None => {
                tracing::warn!(
                    "Could not get cluster as string from association: {:?}",
                    cluster
                );
                return Err(Error::Call(
                    "Could not get cluster as string from association".to_string(),
                ));
            }
        };

        Ok(SlurmAssociation {
            user: user.to_string(),
            account: clean_account_name(account)?,
            cluster: cluster.to_string(),
        })
    }

    pub fn user(&self) -> &str {
        &self.user
    }

    pub fn account(&self) -> &str {
        &self.account
    }

    pub fn cluster(&self) -> &str {
        &self.cluster
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
        let default_account =
            clean_account_name(match mapping.local_group().starts_with("group.") {
                //if it starts with "group.X" then return "X" as this is legacy account
                true => mapping
                    .local_group()
                    .split('.')
                    .nth(1)
                    .unwrap_or(mapping.local_group()),
                false => mapping.local_group(),
            })?;

        Ok(SlurmUser {
            name: mapping.local_user().unix()?.to_string(),
            default_account: Some(default_account),
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SlurmNode {
    cpus: u64,
    gpus: u64,
    mem: u64,
    billing: u64,
}

impl Display for SlurmNode {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "SlurmNode {{ cpus: {}, gpus: {}, mem: {}, billing: {} }}",
            self.cpus(),
            self.gpus(),
            self.mem(),
            self.billing()
        )
    }
}

impl SlurmNode {
    fn new(cpus: u64, gpus: u64, mem: u64, billing: u64) -> Self {
        SlurmNode {
            cpus,
            gpus,
            mem,
            billing,
        }
    }

    pub fn construct(value: &serde_json::Value) -> Result<Self, Error> {
        let cpus = match value.get("cpus") {
            Some(cpus) => match cpus.as_u64() {
                Some(cpus) => cpus,
                None => {
                    tracing::warn!("Could not get cpus as u64 from node: {:?}", cpus);
                    return Err(Error::Call(
                        "Could not get cpus as u64 from node".to_string(),
                    ));
                }
            },
            None => 0,
        };

        let gpus = match value.get("gpus") {
            Some(gpus) => match gpus.as_u64() {
                Some(gpus) => gpus,
                None => {
                    tracing::warn!("Could not get gpus as u64 from node: {:?}", gpus);
                    return Err(Error::Call(
                        "Could not get gpus as u64 from node".to_string(),
                    ));
                }
            },
            None => 0,
        };

        let mem = match value.get("mem") {
            Some(mem) => match mem.as_u64() {
                Some(mem) => mem,
                None => {
                    tracing::warn!("Could not get mem as u64 from node: {:?}", mem);
                    return Err(Error::Call(
                        "Could not get mem as u64 from node".to_string(),
                    ));
                }
            },
            None => 0,
        };

        let billing = match value.get("billing") {
            Some(billing) => match billing.as_u64() {
                Some(billing) => billing,
                None => {
                    tracing::warn!("Could not get billing as u64 from node: {:?}", billing);
                    return Err(Error::Call(
                        "Could not get billing as u64 from node".to_string(),
                    ));
                }
            },
            None => 0,
        };

        Ok(SlurmNode::new(cpus, gpus, mem, billing))
    }

    pub fn cpus(&self) -> u64 {
        self.cpus
    }

    pub fn gpus(&self) -> u64 {
        self.gpus
    }

    pub fn mem(&self) -> u64 {
        self.mem
    }

    pub fn billing(&self) -> u64 {
        self.billing
    }

    pub fn has_cpus(&self) -> bool {
        self.cpus > 0
    }

    pub fn has_gpus(&self) -> bool {
        self.gpus > 0
    }

    pub fn has_mem(&self) -> bool {
        self.mem > 0
    }

    pub fn has_billing(&self) -> bool {
        self.billing > 0
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SlurmNodes {
    nodes: HashMap<String, SlurmNode>,
    default: SlurmNode,
}

impl SlurmNodes {
    pub fn new(default: &SlurmNode) -> Self {
        SlurmNodes {
            nodes: HashMap::new(),
            default: default.clone(),
        }
    }

    pub fn set_default(&mut self, default: &SlurmNode) {
        self.default = default.clone();
    }

    pub fn get_default(&self) -> &SlurmNode {
        &self.default
    }

    pub fn set(&mut self, name: &str, node: &SlurmNode) {
        self.nodes.insert(name.to_string(), node.clone());
    }

    pub fn get(&self, name: &str) -> &SlurmNode {
        self.nodes.get(name).unwrap_or(&self.default)
    }
}

fn get_fraction(used: u64, total: u64) -> f64 {
    match total {
        0 => 0.0,
        _ => used as f64 / total as f64,
    }
}

///
/// Which attempt of a job an accounting record describes.
///
/// Slurm keeps one record per attempt, and a requeued job has several. Default
/// `sacct` returns only the most recent, which is why the earlier attempts were
/// invisible to us until `--duplicates` was added. Classifying them keeps the
/// figure we have always reported (`Base`) apart from the consumption that was
/// previously missing (`Requeued`), so that both can be reported and the policy
/// question of what to charge can be settled separately. See
/// `docs/plans/slurm-requeue-accounting-design.md`.
///
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Attempt {
    /// The last attempt of this job within the query window - the record that
    /// default `sacct` would have returned.
    Base,
    /// An attempt superseded by a later one.
    Requeued,
}

///
/// The terminal states we bucket requeue events by, in the order they take
/// precedence when a record reports several.
///
/// `NODE_FAIL` outranks `REQUEUED` deliberately: a node failure that led to a
/// requeue is reported both ways depending on the record, and "the site lost
/// this work" is the more useful of the two answers. `PREEMPTED` outranks it
/// for the same reason - it says the work was given away by policy.
///
/// The list also bounds the key space of the per-state maps in
/// `DailyProjectUsageReport`. Those keys come from Slurm's JSON, and a map keyed
/// on unbounded peer-supplied strings is the growth problem of
/// `docs/specifications/security-review-2.md` (finding R33). Anything not listed
/// here is bucketed as `OTHER` rather than dropped, so the per-state counts
/// still account for every event.
const TERMINAL_STATES: [&str; 14] = [
    "NODE_FAIL",
    "BOOT_FAIL",
    "PREEMPTED",
    "DEADLINE",
    "OUT_OF_MEMORY",
    "TIMEOUT",
    "REQUEUED",
    "CANCELLED",
    "REVOKED",
    "SPECIAL_EXIT",
    "FAILED",
    "COMPLETED",
    "SUSPENDED",
    "RESIZING",
];

/// The bucket for a state Slurm reports that `TERMINAL_STATES` does not name.
const OTHER_TERMINAL_STATE: &str = "OTHER";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SlurmJob {
    id: u64,
    user: String,
    account: String,
    cluster: String,
    node_info: SlurmNode,
    start_time: chrono::DateTime<chrono::Utc>,
    original_start_time: chrono::DateTime<chrono::Utc>,
    eligible_time: chrono::DateTime<chrono::Utc>,
    end_time: chrono::DateTime<chrono::Utc>,
    /// When this attempt was submitted. With `id` this is `slurmdbd`'s own
    /// primary key for the record, so it both orders a job's attempts and keeps
    /// two unrelated jobs apart if a `slurmctld` reset has reused an id.
    submission_time: chrono::DateTime<chrono::Utc>,
    /// How many times this job had been requeued when this attempt ran. Counts
    /// from the job's own beginning, *not* from the query window, so the lowest
    /// value among the records returned for a job is not necessarily zero.
    restart_count: u64,
    duration: u64,
    /// Every state Slurm reports for this record. A requeued attempt reports
    /// something like `["PENDING", "REQUEUED"]`, so keeping only the first
    /// would record an attempt that ran for hours as merely "PENDING".
    states: Vec<String>,
    /// The node Slurm blamed for a `NODE_FAIL`, if it named one.
    failed_node: String,
    /// Which attempt of the job this is - set by `get_consumers`, which is the
    /// only place that can see a job's other attempts.
    attempt: Attempt,
    qos: String,
    nodes: u64,
    cpus: u64,
    gpus: u64,
    memory: u64,
    requested_nodes: u64,
    requested_cpus: u64,
    requested_gpus: u64,
    requested_memory: u64,
    energy: u64,
    billing: u64,
    requested_billing: u64,
}

impl Display for SlurmJob {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "SlurmJob {{ id: {}, attempt: {:?}, restart_count: {}, user: {}, account: {}, cluster: {}, node_info: {}, start: {}, end: {}, duration: {}s, total_duration: {}s state: {}, qos: {}, nodes: {}, cpus: {}, gpus: {}, memory: {}, requested_nodes: {}, requested_cpus: {}, requested_gpus: {}, requested_memory: {}, energy: {}, billing: {}, requested_billing: {}, node_fraction: {}, billed_node_seconds: {} }}",
            self.id(),
            self.attempt(),
            self.restart_count(),
            self.user(),
            self.account(),
            self.cluster(),
            self.node_info(),
            self.start_time(),
            self.end_time(),
            self.duration().num_seconds(),
            self.total_duration().num_seconds(),
            self.terminal_state(),
            self.qos(),
            self.nodes(),
            self.cpus(),
            self.gpus(),
            self.memory(),
            self.requested_nodes(),
            self.requested_cpus(),
            self.requested_gpus(),
            self.requested_memory(),
            self.energy(),
            self.billing(),
            self.requested_billing(),
            self.node_fraction(),
            self.billed_node_seconds()
        )
    }
}

impl SlurmJob {
    pub fn construct(value: &serde_json::Value, nodeinfos: &SlurmNodes) -> Result<Self, Error> {
        let id = match value.get("job_id") {
            Some(id) => match id.as_u64() {
                Some(id) => id,
                None => {
                    tracing::warn!("Could not get id as u64 from job: {:?}", id);
                    return Err(Error::Call("Could not get id as u64 from job".to_string()));
                }
            },
            None => {
                tracing::warn!("Could not get id from job: {:?}", value);
                return Err(Error::Call("Could not get id from job".to_string()));
            }
        };

        let user = match value.get("user") {
            Some(user) => match user.as_str() {
                Some(user) => user.to_string(),
                None => {
                    tracing::warn!("Could not get user as string from job: {:?}", user);
                    return Err(Error::Call(
                        "Could not get user as string from job".to_string(),
                    ));
                }
            },
            None => {
                tracing::warn!("Could not get user from job: {:?}", value);
                return Err(Error::Call("Could not get user from job".to_string()));
            }
        };

        let account = match value.get("account") {
            Some(account) => match account.as_str() {
                Some(account) => account.to_string(),
                None => {
                    tracing::warn!("Could not get account as string from job: {:?}", account);
                    return Err(Error::Call(
                        "Could not get account as string from job".to_string(),
                    ));
                }
            },
            None => {
                tracing::warn!("Could not get account from job: {:?}", value);
                return Err(Error::Call("Could not get account from job".to_string()));
            }
        };

        let cluster = match value.get("cluster") {
            Some(cluster) => match cluster.as_str() {
                Some(cluster) => cluster.to_string(),
                None => {
                    tracing::warn!("Could not get cluster as string from job: {:?}", cluster);
                    return Err(Error::Call(
                        "Could not get cluster as string from job".to_string(),
                    ));
                }
            },
            None => {
                tracing::warn!("Could not get cluster from job: {:?}", value);
                return Err(Error::Call("Could not get cluster from job".to_string()));
            }
        };

        let node_names: String = match value.get("nodes") {
            Some(nodes) => match nodes.as_str() {
                Some(nodes) => nodes.to_string(),
                None => {
                    tracing::warn!("Could not get nodes as string from job: {:?}", nodes);
                    return Err(Error::Call(
                        "Could not get nodes as string from job".to_string(),
                    ));
                }
            },
            None => {
                tracing::warn!("Could not get nodes from job: {:?}", value);
                return Err(Error::Call("Could not get nodes from job".to_string()));
            }
        };

        let node_info = nodeinfos.get(&node_names).clone();

        let time = match value.get("time") {
            Some(time) => time,
            None => {
                tracing::warn!("Could not get time from job: {:?}", value);
                return Err(Error::Call("Could not get time from job".to_string()));
            }
        };

        let start_time = match time.get("start") {
            Some(start_time) => match start_time.as_i64() {
                Some(start_time) => match chrono::Utc.timestamp_opt(start_time, 0).single() {
                    Some(start_time) => start_time,
                    None => {
                        // Slurm can return nonsense times for jobs that haven't run - this could confused chrono
                        tracing::warn!("Could not get start_time as DateTime from job");
                        chrono::Utc::now()
                    }
                },
                None => {
                    tracing::warn!("Could not get start_time as i64 from job: {:?}", start_time);
                    return Err(Error::Call(
                        "Could not get start_time as i64 from job".to_string(),
                    ));
                }
            },
            None => {
                tracing::warn!("Could not get start_time from job: {:?}", value);
                return Err(Error::Call("Could not get start_time from job".to_string()));
            }
        };

        let end_time = match time.get("end") {
            Some(end_time) => match end_time.as_i64() {
                Some(end_time) => match chrono::Utc.timestamp_opt(end_time, 0).single() {
                    Some(end_time) => end_time,
                    None => {
                        // Slurm can return nonsense times for jobs that haven't run - this could confused chrono
                        tracing::warn!("Could not get end_time as DateTime from job");
                        chrono::Utc::now()
                    }
                },
                None => {
                    tracing::warn!("Could not get end_time as i64 from job: {:?}", end_time);
                    return Err(Error::Call(
                        "Could not get end_time as i64 from job".to_string(),
                    ));
                }
            },
            None => {
                tracing::warn!("Could not get end_time from job: {:?}", value);
                return Err(Error::Call("Could not get end_time from job".to_string()));
            }
        };

        let eligible_time = match time.get("eligible") {
            Some(eligible_time) => match eligible_time.as_i64() {
                Some(eligible_time) => match chrono::Utc.timestamp_opt(eligible_time, 0).single() {
                    Some(eligible_time) => eligible_time,
                    None => {
                        tracing::warn!("Could not get eligible_time as DateTime from job");
                        start_time
                    }
                },
                None => {
                    tracing::warn!(
                        "Could not get eligible_time as i64 from job: {:?}",
                        eligible_time
                    );
                    start_time
                }
            },
            None => start_time,
        };

        let duration: chrono::Duration = match time.get("elapsed") {
            Some(duration) => match duration.as_i64() {
                Some(duration) => chrono::Duration::seconds(duration),
                None => {
                    tracing::warn!("Could not get duration as u64 from job: {:?}", duration);
                    return Err(Error::Call(
                        "Could not get duration as u64 from job".to_string(),
                    ));
                }
            },
            None => {
                tracing::warn!("Could not get duration from job: {:?}", value);
                return Err(Error::Call("Could not get duration from job".to_string()));
            }
        };

        // cannot have negative durations
        let duration = match duration.num_seconds() >= 0 {
            true => duration,
            false => {
                tracing::warn!("Negative duration for job: {:?}", value);
                chrono::Duration::seconds(0)
            }
        };

        let duration = duration.num_seconds() as u64;

        // Slurm reports `state.current` either as a bare string or as a list -
        // a requeued attempt is `["PENDING", "REQUEUED"]`. Keep the whole set:
        // taking only the first element recorded such an attempt as "PENDING",
        // discarding the one piece of information that says it was requeued.
        let states = match value.get("state") {
            Some(state) => match state.get("current") {
                Some(state) => match state.as_str() {
                    Some(state) => vec![state.to_string()],
                    None => match state.as_array() {
                        Some(state) => {
                            let states: Vec<String> = state
                                .iter()
                                .filter_map(|s| s.as_str().map(|s| s.to_string()))
                                .collect();

                            if states.is_empty() {
                                tracing::warn!(
                                    "Could not get state as string from job: {:?}",
                                    state
                                );
                                return Err(Error::Call(
                                    "Could not get state as string from job".to_string(),
                                ));
                            }

                            states
                        }
                        None => {
                            tracing::warn!("Could not get state as string from job: {:?}", state);
                            return Err(Error::Call(
                                "Could not get state as string from job".to_string(),
                            ));
                        }
                    },
                },
                None => {
                    tracing::warn!("Could not get state from job: {:?}", state);
                    return Err(Error::Call("Could not get state from job".to_string()));
                }
            },
            None => {
                tracing::warn!("Could not get state from job: {:?}", value);
                return Err(Error::Call("Could not get state from job".to_string()));
            }
        };

        // `time.submission` orders a job's attempts. It is not fatal for it to
        // be missing - an older Slurm may not report it - but say so, because
        // without it the ordering falls back to `restart_cnt` alone and a job-id
        // reset can no longer be detected.
        let submission_time = match time.get("submission").and_then(|t| t.as_i64()) {
            Some(submission) => match chrono::Utc.timestamp_opt(submission, 0).single() {
                Some(submission) => submission,
                None => {
                    tracing::warn!("Could not get submission time as DateTime from job");
                    eligible_time
                }
            },
            None => {
                tracing::debug!(
                    "No time.submission for job - falling back to the eligible time \
                     for attempt ordering"
                );
                eligible_time
            }
        };

        // Absent for a job that was never requeued, which is the normal case.
        let restart_count = value
            .get("restart_cnt")
            .and_then(|c| c.as_u64())
            .unwrap_or(0);

        let failed_node = value
            .get("failed_node")
            .and_then(|n| n.as_str())
            .unwrap_or_default()
            .to_string();

        let qos = match value.get("qos") {
            Some(qos) => match qos.as_str() {
                Some(qos) => qos.to_string(),
                None => {
                    tracing::warn!("Could not get qos as string from job: {:?}", qos);
                    return Err(Error::Call(
                        "Could not get qos as string from job".to_string(),
                    ));
                }
            },
            None => {
                tracing::warn!("Could not get qos from job: {:?}", value);
                return Err(Error::Call("Could not get qos from job".to_string()));
            }
        };

        let tres = match value.get("tres") {
            Some(tres) => tres,
            None => {
                tracing::warn!("Could not get tres from job: {:?}", value);
                return Err(Error::Call("Could not get tres from job".to_string()));
            }
        };

        let allocated = match tres.get("allocated") {
            Some(allocated) => match allocated.as_array() {
                Some(allocated) => allocated,
                None => {
                    tracing::warn!(
                        "Could not get allocated as object from job: {:?}",
                        allocated
                    );
                    return Err(Error::Call(
                        "Could not get allocated as object from job".to_string(),
                    ));
                }
            },
            None => {
                tracing::warn!("Could not get allocated from job: {:?}", tres);
                return Err(Error::Call("Could not get allocated from job".to_string()));
            }
        };

        let mut nodes = 0;
        let mut cpus = 0;
        let mut memory = 0;
        let mut gpus = 0;
        let mut energy: u64 = 0;
        let mut billing: u64 = 0;

        for tres in allocated {
            let tres_type = match tres.get("type") {
                Some(tres_type) => match tres_type.as_str() {
                    Some(tres_type) => tres_type,
                    None => {
                        tracing::warn!("Could not get type as string from tres: {:?}", tres);
                        return Err(Error::Call(
                            "Could not get type as string from tres".to_string(),
                        ));
                    }
                },
                None => {
                    tracing::warn!("Could not get type from tres: {:?}", tres);
                    return Err(Error::Call("Could not get type from tres".to_string()));
                }
            };

            let name = match tres.get("name") {
                Some(name) => match name.as_str() {
                    Some(name) => name,
                    None => {
                        tracing::warn!("Could not get name as string from tres: {:?}", tres);
                        return Err(Error::Call(
                            "Could not get name as string from tres".to_string(),
                        ));
                    }
                },
                None => {
                    tracing::warn!("Could not get name from tres: {:?}", tres);
                    return Err(Error::Call("Could not get name from tres".to_string()));
                }
            };

            let count: u64 = match tres.get("count") {
                Some(count) => match count.as_i64() {
                    Some(count) => match count >= 0 {
                        true => count as u64,
                        false => 0, // slurm uses negative numbers to signify not available
                    },
                    None => {
                        tracing::warn!("Could not get count as u64 from tres: {:?}", tres);
                        return Err(Error::Call(
                            "Could not get count as u64 from tres".to_string(),
                        ));
                    }
                },
                None => {
                    tracing::warn!("Could not get count from tres: {:?}", tres);
                    return Err(Error::Call("Could not get count from tres".to_string()));
                }
            };

            match tres_type {
                "cpu" => cpus += count,
                "mem" => memory += count,
                "gres" => match name {
                    "gpu" => gpus += count,
                    _ => {
                        tracing::warn!("Unknown gres name: {}", name);
                    }
                },
                "node" => nodes += count,
                "energy" => energy += count,
                "billing" => billing += count,
                _ => {
                    tracing::warn!("Unknown tres type: {}", tres_type);
                }
            }
        }

        let requested = match tres.get("requested") {
            Some(requested) => match requested.as_array() {
                Some(requested) => requested,
                None => {
                    tracing::warn!(
                        "Could not get requested as object from job: {:?}",
                        allocated
                    );
                    return Err(Error::Call(
                        "Could not get requested as object from job".to_string(),
                    ));
                }
            },
            None => {
                tracing::warn!("Could not get requested from job: {:?}", tres);
                return Err(Error::Call("Could not get requested from job".to_string()));
            }
        };

        let mut requested_nodes = 0;
        let mut requested_cpus = 0;
        let mut requested_memory = 0;
        let mut requested_gpus = 0;
        let mut requested_billing: u64 = 0;

        for tres in requested {
            let tres_type = match tres.get("type") {
                Some(tres_type) => match tres_type.as_str() {
                    Some(tres_type) => tres_type,
                    None => {
                        tracing::warn!("Could not get type as string from tres: {:?}", tres);
                        return Err(Error::Call(
                            "Could not get type as string from tres".to_string(),
                        ));
                    }
                },
                None => {
                    tracing::warn!("Could not get type from tres: {:?}", tres);
                    return Err(Error::Call("Could not get type from tres".to_string()));
                }
            };

            let count: u64 = match tres.get("count") {
                Some(count) => match count.as_i64() {
                    Some(count) => match count >= 0 {
                        true => count as u64,
                        false => 0, // slurm uses negative numbers to signify not available
                    },
                    None => {
                        tracing::warn!("Could not get count as u64 from tres: {:?}", tres);
                        return Err(Error::Call(
                            "Could not get count as u64 from tres".to_string(),
                        ));
                    }
                },
                None => {
                    tracing::warn!("Could not get count from tres: {:?}", tres);
                    return Err(Error::Call("Could not get count from tres".to_string()));
                }
            };

            let name = match tres.get("name") {
                Some(name) => match name.as_str() {
                    Some(name) => name,
                    None => {
                        tracing::warn!("Could not get name as string from tres: {:?}", tres);
                        return Err(Error::Call(
                            "Could not get name as string from tres".to_string(),
                        ));
                    }
                },
                None => {
                    tracing::warn!("Could not get name from tres: {:?}", tres);
                    return Err(Error::Call("Could not get name from tres".to_string()));
                }
            };

            match tres_type {
                "cpu" => requested_cpus += count,
                "mem" => requested_memory += count,
                "gres" => match name {
                    "gpu" => requested_gpus += count,
                    _ => {
                        tracing::warn!("Unknown gres name: {}", name);
                    }
                },
                "node" => requested_nodes += count,
                "billing" => requested_billing += count,
                _ => {
                    tracing::warn!("Unknown tres type: {}", tres_type);
                }
            }
        }

        Ok(SlurmJob {
            id,
            user,
            account,
            cluster,
            node_info,
            start_time,
            original_start_time: start_time,
            eligible_time,
            end_time,
            submission_time,
            restart_count,
            duration,
            states,
            failed_node,
            // `get_consumers` reclassifies once it can see the job's other
            // attempts; a job constructed on its own is its own last attempt.
            attempt: Attempt::Base,
            qos,
            nodes,
            cpus,
            gpus,
            memory,
            requested_nodes,
            requested_cpus,
            requested_gpus,
            requested_memory,
            energy,
            billing,
            requested_billing,
        })
    }

    ///
    /// Construct a list of SlurmJobs from a JSON value
    /// Note this skips jobs that have not consumed any resource
    /// (i.e. have a duration of 0). If you want these jobs, you
    /// should contruct each job individually
    ///
    /// With `sacct --duplicates` the response holds one record per *attempt* of
    /// a requeued job, so each record is also classified as the job's last
    /// attempt within this window (`Attempt::Base`) or a superseded one
    /// (`Attempt::Requeued`) - see `classify_attempts`.
    ///
    pub fn get_consumers(
        value: &serde_json::Value,
        start_time: &chrono::DateTime<chrono::Utc>,
        end_time: &chrono::DateTime<chrono::Utc>,
        slurm_nodes: &SlurmNodes,
    ) -> Result<Vec<SlurmJob>, Error> {
        if start_time > end_time {
            return Err(Error::Call(format!(
                "Start time '{}' is after end time '{}'",
                start_time, end_time
            )));
        }

        let jobs = match value.get("jobs") {
            Some(jobs) => match jobs.as_array() {
                Some(jobs) => {
                    let mut slurm_jobs: Vec<SlurmJob> = Vec::new();

                    // Construct everything first, with no clipping and no
                    // filtering. The order of these steps is load-bearing:
                    // classification has to see every record, including the
                    // zero-duration ones dropped below. A job whose last
                    // attempt was cancelled before it ran has a zero-duration
                    // final record, and dropping it first would promote an
                    // earlier attempt to `Base` - which would move hours of
                    // previously unreported usage into the figure that is
                    // supposed to stay unchanged.
                    for job in jobs {
                        match SlurmJob::construct(job, slurm_nodes) {
                            Ok(job) => slurm_jobs.push(job),
                            Err(e) => {
                                tracing::warn!("Could not construct job from {}: {}", job, e);
                            }
                        }
                    }

                    Self::classify_attempts(&mut slurm_jobs);

                    // now clip each record to the query window, and drop the
                    // ones that consumed nothing within it
                    let mut consumers: Vec<SlurmJob> = Vec::new();

                    for mut job in slurm_jobs {
                        if job.start_time < *start_time {
                            job.start_time = *start_time;
                        } else if job.start_time > *end_time {
                            // job was likely cancelled
                            job.start_time = *end_time;
                        }

                        if job.end_time > *end_time || job.end_time < *start_time {
                            job.end_time = *end_time;
                        }

                        if job.duration().num_seconds() > 0 {
                            tracing::debug!("Recording job {}", job);
                            consumers.push(job)
                        }
                    }

                    consumers
                }
                None => {
                    tracing::warn!("Jobs is not an array: {:?}", jobs);
                    return Err(Error::Call("Jobs is not an array".to_string()));
                }
            },
            None => Vec::new(),
        };

        Ok(jobs)
    }

    ///
    /// Mark each record as the last attempt of its job within this response, or
    /// as an attempt superseded by a later one.
    ///
    /// Records are grouped by job id and ordered by submission time, which with
    /// the id is `slurmdbd`'s own key for a record. Within a group the restart
    /// count must increase from one attempt to the next; where it does not, the
    /// records belong to two different jobs that share an id because
    /// `slurmctld` was reset, and each gets its own last attempt. Without that
    /// split the older job's final attempt would be charged to the requeue
    /// bucket of the newer one.
    ///
    fn classify_attempts(jobs: &mut [SlurmJob]) {
        // job id -> indices of that job's records, in the order they appear
        let mut groups: HashMap<u64, Vec<usize>> = HashMap::new();

        for (index, job) in jobs.iter().enumerate() {
            groups.entry(job.id()).or_default().push(index);
        }

        for (id, mut indices) in groups {
            if indices.len() == 1 {
                // the common case: one record, so it is its own last attempt
                continue;
            }

            indices.sort_by_key(|index| {
                jobs.get(*index)
                    .map(|job| (job.submission_time, job.restart_count))
                    .unwrap_or_default()
            });

            // split into chains of attempts of the same job
            let mut chains: Vec<Vec<usize>> = Vec::new();
            let mut previous_restart: Option<u64> = None;

            for index in indices {
                let Some(job) = jobs.get(index) else {
                    continue;
                };

                let starts_new_chain = match previous_restart {
                    Some(previous) => job.restart_count <= previous,
                    None => true,
                };

                if starts_new_chain {
                    chains.push(vec![index]);
                } else if let Some(chain) = chains.last_mut() {
                    chain.push(index);
                }

                previous_restart = Some(job.restart_count);
            }

            if chains.len() > 1 {
                let submissions: Vec<String> = chains
                    .iter()
                    .filter_map(|chain| chain.first())
                    .filter_map(|index| jobs.get(*index))
                    .map(|job| job.submission_time().to_rfc3339())
                    .collect();

                tracing::warn!(
                    "Job id {} has {} sets of attempts whose restart counts do not form a \
                     single sequence, first submitted at {}. Treating them as separate jobs \
                     that share an id, which is what a slurmctld job-id reset looks like.",
                    id,
                    chains.len(),
                    submissions.join(", ")
                );
            }

            for chain in chains {
                let Some((last, superseded)) = chain.split_last() else {
                    continue;
                };

                if let Some(job) = jobs.get_mut(*last) {
                    job.attempt = Attempt::Base;
                }

                for index in superseded {
                    if let Some(job) = jobs.get_mut(*index) {
                        job.attempt = Attempt::Requeued;
                    }
                }
            }
        }
    }

    pub fn id(&self) -> u64 {
        self.id
    }

    pub fn user(&self) -> &str {
        &self.user
    }

    pub fn account(&self) -> &str {
        &self.account
    }

    pub fn cluster(&self) -> &str {
        &self.cluster
    }

    pub fn node_info(&self) -> &SlurmNode {
        &self.node_info
    }

    pub fn start_time(&self) -> &chrono::DateTime<chrono::Utc> {
        &self.start_time
    }

    pub fn original_start_time(&self) -> &chrono::DateTime<chrono::Utc> {
        &self.original_start_time
    }

    pub fn eligible_time(&self) -> &chrono::DateTime<chrono::Utc> {
        &self.eligible_time
    }

    /// The time the job spent waiting in the queue before it started running.
    /// Clamped to zero if eligible_time is after start_time (handles bogus Slurm timestamps).
    ///
    /// Measured from the *unclipped* start: how long an attempt queued is a
    /// property of the attempt, not of the window it is being reported in.
    /// Using the clipped start would report an attempt that began before the
    /// window as having waited until the window opened. That never showed for a
    /// job count, which only counts attempts that started inside the window, so
    /// for those the two starts are the same - but a requeue is counted in the
    /// window where it happened, which is not where the attempt began.
    pub fn wait_time(&self) -> chrono::Duration {
        let wait = self
            .original_start_time
            .signed_duration_since(self.eligible_time());
        if wait.num_seconds() < 0 {
            chrono::Duration::seconds(0)
        } else {
            wait
        }
    }

    pub fn end_time(&self) -> &chrono::DateTime<chrono::Utc> {
        &self.end_time
    }

    pub fn duration(&self) -> chrono::Duration {
        match self.duration > 0 {
            false => chrono::Duration::seconds(0),
            // use the actual difference between start and end times
            // as these are trimmed to the query that generated the job
            true => self.end_time.signed_duration_since(self.start_time),
        }
    }

    pub fn total_duration(&self) -> chrono::Duration {
        // return the total duration of the job, including
        // consumption outside the query used to generate the job
        chrono::Duration::seconds(self.duration as i64)
    }

    /// Every state Slurm reported for this record.
    pub fn states(&self) -> &[String] {
        &self.states
    }

    ///
    /// The state this attempt ended in, bucketed for reporting.
    ///
    /// Picks the highest-precedence entry of `TERMINAL_STATES` present in
    /// `states()`, so that a node failure is reported as `NODE_FAIL` rather
    /// than as the `PENDING` that a requeued record leads with. A state Slurm
    /// reports that we do not know is bucketed as `OTHER` rather than dropped,
    /// which keeps the per-state counts accounting for every event and keeps
    /// the key space of the per-state maps bounded.
    ///
    pub fn terminal_state(&self) -> &'static str {
        for candidate in TERMINAL_STATES {
            if self
                .states
                .iter()
                .any(|state| state.eq_ignore_ascii_case(candidate))
            {
                return candidate;
            }
        }

        OTHER_TERMINAL_STATE
    }

    /// When this attempt was submitted.
    pub fn submission_time(&self) -> &chrono::DateTime<chrono::Utc> {
        &self.submission_time
    }

    /// How many times the job had been requeued when this attempt ran.
    pub fn restart_count(&self) -> u64 {
        self.restart_count
    }

    /// Whether this record is the job's last attempt in the query window, or
    /// one superseded by a requeue.
    pub fn attempt(&self) -> Attempt {
        self.attempt
    }

    /// True if this attempt was superseded by a requeue - i.e. its usage was
    /// invisible before requeue accounting.
    pub fn is_requeued_attempt(&self) -> bool {
        self.attempt == Attempt::Requeued
    }

    /// The node Slurm blamed for a `NODE_FAIL`, if it named one.
    pub fn failed_node(&self) -> &str {
        &self.failed_node
    }

    pub fn qos(&self) -> &str {
        &self.qos
    }

    pub fn nodes(&self) -> u64 {
        self.nodes
    }

    pub fn cpus(&self) -> u64 {
        self.cpus
    }

    pub fn gpus(&self) -> u64 {
        self.gpus
    }

    pub fn memory(&self) -> u64 {
        self.memory
    }

    pub fn requested_nodes(&self) -> u64 {
        self.requested_nodes
    }

    pub fn requested_cpus(&self) -> u64 {
        self.requested_cpus
    }

    pub fn requested_gpus(&self) -> u64 {
        self.requested_gpus
    }

    pub fn requested_memory(&self) -> u64 {
        self.requested_memory
    }

    pub fn energy(&self) -> u64 {
        self.energy
    }

    pub fn billing(&self) -> u64 {
        self.billing
    }

    pub fn requested_billing(&self) -> u64 {
        self.requested_billing
    }

    pub fn requested_node_fraction(&self) -> f64 {
        // find the maximum fraction of the node that was used
        let cpu_fraction = get_fraction(self.requested_cpus, self.node_info.cpus());
        let gpu_fraction = get_fraction(self.requested_gpus, self.node_info.gpus());
        let memory_fraction = get_fraction(self.requested_memory, self.node_info.mem());
        let billing_fraction = get_fraction(self.requested_billing, self.node_info.billing());

        cpu_fraction
            .max(gpu_fraction)
            .max(memory_fraction)
            .max(billing_fraction)
    }

    pub fn node_fraction(&self) -> f64 {
        // find the maximum fraction of the node that was used
        let cpu_fraction = get_fraction(self.cpus, self.node_info.cpus());
        let gpu_fraction = get_fraction(self.gpus, self.node_info.gpus());
        let memory_fraction = get_fraction(self.memory, self.node_info.mem());
        let billing_fraction = get_fraction(self.billing, self.node_info.billing());

        cpu_fraction
            .max(gpu_fraction)
            .max(memory_fraction)
            .max(billing_fraction)
    }

    pub fn billed_node_fraction(&self) -> f64 {
        let actual_node_fraction = self.node_fraction();
        let requested_node_fraction = self.requested_node_fraction();

        // write a warning to the log if the actual node fraction is greater than the requested
        // node fraction. This indicates that slurm accepted a job that requested too few resources,
        // and then had to uprate it to the actual amount
        if requested_node_fraction < actual_node_fraction {
            // Note: we log the job id instead of self to avoid infinite recursion,
            // since Display::fmt calls billed_node_seconds which calls this function
            tracing::warn!(
                "Job {} used more resources than requested: {} > {}",
                self.id,
                actual_node_fraction,
                requested_node_fraction
            );
        }

        actual_node_fraction
    }

    pub fn billed_node_seconds(&self) -> u64 {
        let billed_seconds =
            (self.duration().num_seconds() as f64 * self.billed_node_fraction()).ceil() as u64;

        if billed_seconds == 0 && self.duration().num_seconds() > 0 {
            tracing::warn!(
                "Job {} has a non-zero duration but zero billed node seconds - this may indicate an issue with the slurm configuration",
                self.id
            );

            // return at least 1 second to avoid issues with zero billing
            1
        } else {
            billed_seconds
        }
    }

    pub fn cpu_seconds(&self) -> u64 {
        let cpu_seconds = self.cpus * self.duration().num_seconds() as u64;

        if cpu_seconds == 0 && self.cpus > 0 && self.duration().num_seconds() > 0 {
            tracing::warn!(
                "Job {} has non-zero cpus and duration but zero cpu seconds - this may indicate an issue with the slurm configuration",
                self.id
            );

            // return at least 1 second to avoid issues with zero billing
            1
        } else {
            cpu_seconds
        }
    }

    pub fn gpu_seconds(&self) -> u64 {
        let gpu_seconds = self.gpus * self.duration().num_seconds() as u64;

        if gpu_seconds == 0 && self.gpus > 0 && self.duration().num_seconds() > 0 {
            tracing::warn!(
                "Job {} has non-zero gpus and duration but zero gpu seconds - this may indicate an issue with the slurm configuration",
                self.id
            );

            // return at least 1 second to avoid issues with zero billing
            1
        } else {
            gpu_seconds
        }
    }

    pub fn memory_seconds(&self) -> u64 {
        let memory_seconds = self.memory * self.duration().num_seconds() as u64;

        if memory_seconds == 0 && self.memory > 0 && self.duration().num_seconds() > 0 {
            tracing::warn!(
                "Job {} has non-zero memory and duration but zero memory seconds - this may indicate an issue with the slurm configuration",
                self.id
            );

            // return at least 1 second to avoid issues with zero billing
            1
        } else {
            memory_seconds
        }
    }

    pub fn billing_seconds(&self) -> u64 {
        let billing_seconds = self.billing * self.duration().num_seconds() as u64;

        if billing_seconds == 0 && self.billing > 0 && self.duration().num_seconds() > 0 {
            tracing::warn!(
                "Job {} has non-zero billing and duration but zero billing seconds - this may indicate an issue with the slurm configuration",
                self.id
            );

            // return at least 1 second to avoid issues with zero billing
            1
        } else {
            billing_seconds
        }
    }
}

pub async fn connect(
    server: &str,
    user: &str,
    token_command: &str,
    token_lifespan: u32,
    num_servers: u64,
) -> Result<(), Error> {
    // make sure that the token lifespan is at least 10 seconds
    let token_lifespan = match token_lifespan < 10 {
        true => 10,
        false => token_lifespan,
    };

    // make sure that the number of servers is at least 1
    let num_servers = match num_servers < 1 {
        true => 1,
        false => num_servers,
    };

    let servers = vec![server.to_string(); num_servers as usize];
    let users = vec![user.to_string(); num_servers as usize];
    let token_commands = vec![token_command.to_string(); num_servers as usize];
    let token_lifespans = vec![token_lifespan; num_servers as usize];

    // initialise with a single server
    initialise_servers(&servers, &users, &token_commands, &token_lifespans).await?;

    // try to login to make sure we can connect
    let expires = Utc::now() + chrono::Duration::minutes(1);
    get_connected_server(&expires).await?;

    Ok(())
}

pub async fn add_user(user: &UserMapping, expires: &chrono::DateTime<Utc>) -> Result<(), Error> {
    // get a lock for this user, as only a single task should be adding
    // or removing this user at the same time
    let now = chrono::Utc::now();

    let _guard = loop {
        match cache::get_user_mutex(user.user()).await?.try_lock_owned() {
            Ok(guard) => break guard,
            Err(_) => {
                if chrono::Utc::now().signed_duration_since(now).num_seconds() > 5 {
                    tracing::warn!(
                        "Could not get lock to add user {} - another task is adding or removing.",
                        user
                    );

                    return Err(Error::Locked(format!(
                        "Could not get lock to add user {} - another task is adding or removing.",
                        user
                    )));
                }

                assert_not_expired(expires)?;
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }
        };
    };

    let user: SlurmUser = get_user_create_if_not_exists(user, expires).await?;

    tracing::info!("Added user: {}", user);

    Ok(())
}

pub async fn add_project(
    project: &ProjectMapping,
    expires: &chrono::DateTime<Utc>,
) -> Result<(), Error> {
    assert_not_expired(expires)?;

    // get a lock for this project, as only a single task should be adding
    // or removing this project at the same time
    let now = chrono::Utc::now();

    let _guard = loop {
        match cache::get_project_mutex(project.project())
            .await?
            .try_lock_owned()
        {
            Ok(guard) => break guard,
            Err(_) => {
                if chrono::Utc::now().signed_duration_since(now).num_seconds() > 5 {
                    tracing::warn!(
                        "Could not get lock to add project {} - another task is adding or removing.",
                        project
                    );

                    return Err(Error::Locked(format!(
                        "Could not get lock to add project {} - another task is adding or removing.",
                        project
                    )));
                }

                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }
        };
    };

    let account = SlurmAccount::from_mapping(project)?;

    let account = get_account_create_if_not_exists(&account, expires).await?;

    tracing::info!("Added account: {}", account);

    Ok(())
}

pub async fn get_usage_report(
    project: &ProjectMapping,
    dates: &DateRange,
    expires: &chrono::DateTime<Utc>,
) -> Result<ProjectUsageReport, Error> {
    assert_not_expired(expires)?;

    // Call the sacctmgr version
    sacctmgr::get_usage_report(project, dates, expires).await
}

pub async fn get_limit(
    project: &ProjectMapping,
    expires: &chrono::DateTime<Utc>,
) -> Result<Usage, Error> {
    assert_not_expired(expires)?;

    // Call the sacctmgr version
    sacctmgr::get_limit(project, expires).await
}

pub async fn set_limit(
    project: &ProjectMapping,
    limit: &Usage,
    expires: &chrono::DateTime<Utc>,
) -> Result<Usage, Error> {
    assert_not_expired(expires)?;

    // Call the sacctmgr version
    sacctmgr::set_limit(project, limit, expires).await
}

///
/// Fixture records shared by the tests in this crate.
///
/// Lives here rather than in `mod tests` because the accumulation the reports
/// are built with is in `sacctmgr`, and it has to be tested against the same
/// records that `get_consumers` produces - the interesting cases are the ones
/// where the two disagree about what a record means.
///
#[cfg(test)]
pub(crate) mod test_fixture {
    use super::*;

    /// The two consecutive daily windows the fixture in
    /// `tests/data/sacct-requeued-jobs.json` covers: 2026-03-01, and 2026-03-02
    /// for the job that spans midnight.
    const DAY_ONE_START: i64 = 1772323200;
    const DAY_TWO_START: i64 = 1772409600;
    const DAY_THREE_START: i64 = 1772496000;

    pub fn window() -> (chrono::DateTime<chrono::Utc>, chrono::DateTime<chrono::Utc>) {
        day_one()
    }

    pub fn day_one() -> (chrono::DateTime<chrono::Utc>, chrono::DateTime<chrono::Utc>) {
        (
            chrono::Utc.timestamp_opt(DAY_ONE_START, 0).unwrap(),
            chrono::Utc.timestamp_opt(DAY_TWO_START, 0).unwrap(),
        )
    }

    pub fn day_two() -> (chrono::DateTime<chrono::Utc>, chrono::DateTime<chrono::Utc>) {
        (
            chrono::Utc.timestamp_opt(DAY_TWO_START, 0).unwrap(),
            chrono::Utc.timestamp_opt(DAY_THREE_START, 0).unwrap(),
        )
    }

    /// `testnode` is a whole 128-cpu, 128-billing node, so a job allocating all
    /// of it bills one node-second per second and the expected values below are
    /// the elapsed times themselves.
    ///
    /// The *default* node is deliberately nothing like it. `SlurmNodes::get`
    /// falls back to the default for a node name it does not know, silently, so
    /// if the fixture's node names and this map ever drift apart every number
    /// here changes. Making the fallback wildly different means the tests fail
    /// rather than quietly checking the wrong arithmetic.
    pub fn test_nodes() -> SlurmNodes {
        let default = SlurmNode::construct(&serde_json::json!({
            "cpus": 1, "gpus": 0, "mem": 1, "billing": 1
        }))
        .unwrap();

        let node = SlurmNode::construct(&serde_json::json!({
            "cpus": 128, "gpus": 0, "mem": 0, "billing": 128
        }))
        .unwrap();

        let mut nodes = SlurmNodes::new(&default);
        nodes.set("testnode", &node);
        nodes
    }

    pub fn fixture() -> serde_json::Value {
        serde_json::from_str(include_str!("../tests/data/sacct-requeued-jobs.json")).unwrap()
    }

    /// The fixture's records for one job id, in the order `get_consumers`
    /// returned them.
    pub fn records_for(jobs: &[SlurmJob], id: u64) -> Vec<&SlurmJob> {
        jobs.iter().filter(|job| job.id() == id).collect()
    }

    pub fn usage_of(jobs: &[SlurmJob], id: u64, attempt: Attempt) -> u64 {
        records_for(jobs, id)
            .iter()
            .filter(|job| job.attempt() == attempt)
            .map(|job| job.billed_node_seconds())
            .sum()
    }

    ///
    /// The fixture records `sacct` would return for a query over this window.
    ///
    /// `--starttime`/`--endtime` select the records that *overlap* the window,
    /// which is what decides whether a job's other attempts are visible at all -
    /// and therefore whether a superseded attempt can be recognised as one. The
    /// fixture holds every record for every window, so a test that skipped this
    /// filter would hand `get_consumers` attempts the real query could not have
    /// seen, and would not be testing the case that matters.
    ///
    /// A record that never started is placed by its submission time, as `sacct`
    /// places it - its `start` is zero, which is not a time it existed at.
    ///
    pub fn records_in_window(
        start_time: &chrono::DateTime<chrono::Utc>,
        end_time: &chrono::DateTime<chrono::Utc>,
    ) -> serde_json::Value {
        let mut fixture = fixture();

        let Some(records) = fixture.get_mut("jobs").and_then(|jobs| jobs.as_array_mut()) else {
            unreachable!("the fixture has a jobs array");
        };

        records.retain(|record| {
            let at = |key: &str| {
                record
                    .get("time")
                    .and_then(|time| time.get(key))
                    .and_then(|value| value.as_i64())
                    .unwrap_or(0)
            };

            let began = at("start").max(at("submission"));
            let ended = at("end");

            began < end_time.timestamp() && ended > start_time.timestamp()
        });

        fixture
    }

    pub fn consumers_for(
        window: (chrono::DateTime<chrono::Utc>, chrono::DateTime<chrono::Utc>),
    ) -> Vec<SlurmJob> {
        let (start, end) = window;
        SlurmJob::get_consumers(
            &records_in_window(&start, &end),
            &start,
            &end,
            &test_nodes(),
        )
        .unwrap()
    }
}

#[cfg(test)]
mod tests {
    use super::test_fixture::*;
    use super::*;

    fn consumers() -> Vec<SlurmJob> {
        consumers_for(day_one())
    }

    #[test]
    fn test_every_attempt_of_a_requeued_job_is_returned_and_classified() {
        // Without `--duplicates` sacct returns one record per job and every
        // earlier attempt of a requeued job is invisible. With it, each job's
        // last attempt in the window is `Base` - the record sacct used to
        // return - and the rest are `Requeued`.
        let jobs = consumers();

        // job 100 was never requeued: one record, and it is its own last attempt
        let job_100 = records_for(&jobs, 100);
        assert_eq!(job_100.len(), 1);
        assert_eq!(job_100[0].attempt(), Attempt::Base);

        // job 200 was requeued once: the completed attempt is the base one
        let job_200 = records_for(&jobs, 200);
        assert_eq!(job_200.len(), 2);
        assert_eq!(usage_of(&jobs, 200, Attempt::Base), 1800);
        assert_eq!(usage_of(&jobs, 200, Attempt::Requeued), 3600);

        // job 300 was requeued twice, so two of its three records are superseded
        let job_300 = records_for(&jobs, 300);
        assert_eq!(job_300.len(), 3);
        assert_eq!(
            job_300
                .iter()
                .filter(|job| job.attempt() == Attempt::Requeued)
                .count(),
            2
        );
        assert_eq!(usage_of(&jobs, 300, Attempt::Base), 300);
        assert_eq!(usage_of(&jobs, 300, Attempt::Requeued), 2700);
    }

    #[test]
    fn test_attempts_are_ordered_rather_than_matched_against_a_zero_restart_count() {
        // `restart_cnt` counts from the job's own beginning, not from the query
        // window, so a job whose earlier attempts fell before the window
        // returns no record with `restart_cnt == 0`. Job 400's records start at
        // 2. Classifying on `restart_cnt == 0` would leave it with no base
        // attempt at all and move its whole final run into the requeue bucket.
        let jobs = consumers();
        let job_400 = records_for(&jobs, 400);

        assert_eq!(job_400.len(), 2);
        assert!(job_400.iter().all(|job| job.restart_count() >= 2));

        let base: Vec<&&SlurmJob> = job_400
            .iter()
            .filter(|job| job.attempt() == Attempt::Base)
            .collect();

        assert_eq!(base.len(), 1);
        assert_eq!(base[0].restart_count(), 3);

        // submission time is what the ordering keys on - with the job id it is
        // slurmdbd's own key for a record - and it must agree with the restart
        // count on a job whose attempts form a single chain
        let superseded = job_400
            .iter()
            .find(|job| job.attempt() == Attempt::Requeued)
            .unwrap();
        assert!(superseded.submission_time() < base[0].submission_time());
        assert_eq!(usage_of(&jobs, 400, Attempt::Base), 600);
        assert_eq!(usage_of(&jobs, 400, Attempt::Requeued), 1800);
    }

    #[test]
    fn test_a_zero_duration_final_attempt_does_not_promote_an_earlier_one() {
        // The worst case of the original bug. Job 500 ran for two hours, was
        // requeued, and its final attempt was cancelled before it ran - so the
        // only record default sacct returned had zero elapsed, was dropped as a
        // non-consumer, and the job was reported as having used nothing.
        //
        // Classification therefore has to happen before the zero-duration
        // filter: drop that record first and the earlier attempt becomes the
        // job's last surviving one, which would move two hours of previously
        // unreported usage into the figure that is supposed to stay unchanged.
        let jobs = consumers();

        assert_eq!(usage_of(&jobs, 500, Attempt::Base), 0);
        assert_eq!(usage_of(&jobs, 500, Attempt::Requeued), 7200);
    }

    #[test]
    fn test_a_reused_job_id_is_not_treated_as_a_requeue() {
        // Job 600 is two unrelated jobs that share an id because slurmctld was
        // reset - both records have `restart_cnt` 0, which no single job's
        // attempts can. Neither may be classed as a requeue of the other, or
        // the older job's usage would be charged to the newer one's requeue
        // bucket.
        let jobs = consumers();
        let job_600 = records_for(&jobs, 600);

        assert_eq!(job_600.len(), 2);
        assert!(job_600.iter().all(|job| job.attempt() == Attempt::Base));
        assert_eq!(usage_of(&jobs, 600, Attempt::Requeued), 0);
        assert_eq!(usage_of(&jobs, 600, Attempt::Base), 5400);
    }

    #[test]
    fn test_an_attempt_is_clipped_to_the_window_but_keeps_its_real_start() {
        // Job 700's first attempt began before the window and was requeued
        // inside it. Its usage is clipped to the part inside, but its original
        // start stays outside - which is what stops it being counted as an
        // event in a window it did not start in.
        let (start, _) = window();
        let jobs = consumers();
        let job_700 = records_for(&jobs, 700);

        let superseded: Vec<&&SlurmJob> = job_700
            .iter()
            .filter(|job| job.attempt() == Attempt::Requeued)
            .collect();

        assert_eq!(superseded.len(), 1);
        // ran for two hours, of which one hour was inside the window
        assert_eq!(superseded[0].total_duration().num_seconds(), 7200);
        assert_eq!(superseded[0].billed_node_seconds(), 3600);
        assert!(superseded[0].original_start_time() < &start);
        assert_eq!(usage_of(&jobs, 700, Attempt::Base), 1800);
    }

    #[test]
    fn test_terminal_state_precedence_and_bucketing() {
        let jobs = consumers();

        // a requeued attempt leads with PENDING; reporting it as such would
        // describe an attempt that ran for hours as merely queued
        let job_400 = records_for(&jobs, 400);
        let requeued = job_400
            .iter()
            .find(|job| job.attempt() == Attempt::Requeued)
            .unwrap();
        assert_eq!(requeued.states(), ["PENDING", "REQUEUED"]);
        assert_eq!(requeued.terminal_state(), "REQUEUED");

        // a node failure is reported as such - the site lost the work
        let job_300 = records_for(&jobs, 300);
        assert!(job_300
            .iter()
            .filter(|job| job.attempt() == Attempt::Requeued)
            .all(|job| job.terminal_state() == "NODE_FAIL"));

        // and a state we do not know is bucketed, never dropped, so that the
        // per-state counts still account for every event
        let job_800 = records_for(&jobs, 800);
        let unknown = job_800
            .iter()
            .find(|job| job.attempt() == Attempt::Requeued)
            .unwrap();
        assert_eq!(unknown.terminal_state(), OTHER_TERMINAL_STATE);
    }

    #[test]
    fn test_node_failures_name_the_node_they_lost_the_job_on() {
        // What turns "the project spent this" into "the site lost this" in a
        // charging dispute, and what site monitoring alerts on.
        let jobs = consumers();
        let job_300 = records_for(&jobs, 300);

        let mut failed: Vec<&str> = job_300
            .iter()
            .filter(|job| job.terminal_state() == "NODE_FAIL")
            .map(|job| job.failed_node())
            .collect();
        failed.sort();

        assert_eq!(failed, ["badnode01", "badnode02"]);
    }

    #[test]
    fn test_a_requeue_across_midnight_is_counted_once_on_the_day_it_happened() {
        // The shape almost every real requeue has, and the case the first
        // version of the event count got wrong. Job 900's attempt starts on day
        // one, runs past midnight, and is requeued on day two; the attempt that
        // replaces it never runs, so it has zero elapsed.
        //
        // On day one the successor does not exist yet, so the attempt is the
        // job's last one and is classified `Base` - there is nothing to say it
        // will later be requeued. Only on day two are both records in the same
        // response, and only there can the requeue be seen. Its *start*,
        // though, is on day one, so requiring the record to have started in the
        // window meant this requeue - and nearly every other one, since the
        // attempts that get requeued are the long ones - was never counted at
        // all, while its usage was being reported correctly all along.
        let day_one_jobs = consumers_for(day_one());
        let day_two_jobs = consumers_for(day_two());

        let day_one_records = records_for(&day_one_jobs, 900);
        assert_eq!(day_one_records.len(), 1);
        assert_eq!(day_one_records[0].attempt(), Attempt::Base);

        // on day two the successor is visible, so the attempt is recognised as
        // superseded - and its start is on the day before
        let day_two_records = records_for(&day_two_jobs, 900);
        assert_eq!(day_two_records.len(), 1); // the zero-elapsed successor is not a consumer
        assert_eq!(day_two_records[0].attempt(), Attempt::Requeued);
        assert!(day_two_records[0].original_start_time() < &day_two().0);

        // exactly one window sees the requeue, so counting every superseded
        // record counts the event once - no guard needed, and no double count
        let requeues_seen = |jobs: &[SlurmJob]| {
            jobs.iter()
                .filter(|job| job.id() == 900 && job.attempt() == Attempt::Requeued)
                .count()
        };

        assert_eq!(requeues_seen(&day_one_jobs), 0);
        assert_eq!(requeues_seen(&day_two_jobs), 1);

        // and the usage is split across the two days without being lost or
        // counted twice: twelve hours of it, four on day one and eight on day two
        assert_eq!(usage_of(&day_one_jobs, 900, Attempt::Base), 14400);
        assert_eq!(usage_of(&day_two_jobs, 900, Attempt::Requeued), 28800);
        assert_eq!(
            usage_of(&day_one_jobs, 900, Attempt::Base)
                + usage_of(&day_two_jobs, 900, Attempt::Requeued),
            43200
        );
    }

    #[test]
    fn test_base_usage_equals_what_default_sacct_reported() {
        // The continuity property the whole design rests on: the base figure is
        // what we reported before requeue accounting, so no consumer sees a
        // step change. Simulate default sacct by keeping only the
        // latest-submitted record for each job id, and check the base usage
        // matches.
        //
        // Job 600 is excluded: it is two jobs sharing an id after a job-id
        // reset, and default sacct de-duplicated them down to one, losing the
        // older job's usage entirely. Splitting them is a deliberate
        // improvement on the old figure, not continuity with it.
        let (start, end) = window();
        let nodes = test_nodes();
        let mut fixture = records_in_window(&start, &end);

        {
            let Some(records) = fixture.get_mut("jobs").and_then(|jobs| jobs.as_array_mut()) else {
                unreachable!("the fixture has a jobs array");
            };

            records.retain(|record| record.get("job_id").and_then(|id| id.as_u64()) != Some(600));
        }

        let Some(records) = fixture.get("jobs").and_then(|jobs| jobs.as_array()) else {
            unreachable!("the fixture has a jobs array");
        };

        // what we report now
        let base_usage: u64 = SlurmJob::get_consumers(&fixture, &start, &end, &nodes)
            .unwrap()
            .iter()
            .filter(|job| job.attempt() == Attempt::Base)
            .map(|job| job.billed_node_seconds())
            .sum();

        // what default sacct would have handed us: one record per job id, the
        // most recently submitted
        let mut latest: HashMap<u64, serde_json::Value> = HashMap::new();

        for record in records.iter() {
            let id = record.get("job_id").and_then(|id| id.as_u64()).unwrap();
            let submission = record
                .get("time")
                .and_then(|time| time.get("submission"))
                .and_then(|value| value.as_i64())
                .unwrap();

            let is_later = latest
                .get(&id)
                .and_then(|existing| existing.get("time"))
                .and_then(|time| time.get("submission"))
                .and_then(|value| value.as_i64())
                .is_none_or(|existing| submission > existing);

            if is_later {
                latest.insert(id, record.clone());
            }
        }

        let deduplicated = serde_json::json!({
            "jobs": latest.into_values().collect::<Vec<serde_json::Value>>()
        });

        let legacy_usage: u64 = SlurmJob::get_consumers(&deduplicated, &start, &end, &nodes)
            .unwrap()
            .iter()
            .map(|job| job.billed_node_seconds())
            .sum();

        assert_eq!(base_usage, legacy_usage);
    }

    #[test]
    fn test_base_and_requeue_usage_sum_to_the_true_total() {
        // The other half of the contract: nothing is counted twice and nothing
        // is dropped, so a client that wants the real figure can add them.
        let jobs = consumers();

        let base: u64 = jobs
            .iter()
            .filter(|job| job.attempt() == Attempt::Base)
            .map(|job| job.billed_node_seconds())
            .sum();
        let requeued: u64 = jobs
            .iter()
            .filter(|job| job.attempt() == Attempt::Requeued)
            .map(|job| job.billed_node_seconds())
            .sum();
        let everything: u64 = jobs.iter().map(|job| job.billed_node_seconds()).sum();

        assert_eq!(base, 28800);
        assert_eq!(requeued, 20700);
        assert_eq!(base + requeued, everything);

        // and the requeue share is the point of the exercise - it was all
        // invisible before
        assert_eq!(everything, 49500);
    }

    #[test]
    fn test_only_accounts_in_the_managed_organization_are_managed() {
        // Every mutation path (`set_limit`, `cancel_pending_*_jobs`) gates on
        // `is_managed()` applied to the account *fetched from Slurm*. Before
        // finding R5 the only check was on a locally-constructed account whose
        // organization was hard-wired to the managed one, so it could never
        // fail, and a peer-chosen `local_group` naming any real account on the
        // cluster had its limits rewritten.
        let account_json = |org: &str| {
            serde_json::json!({
                "name": "someproject",
                "description": "a project",
                "organization": org,
                "associations": [{"cluster": "cluster1"}],
            })
        };

        let managed = match SlurmAccount::construct(&account_json(&get_managed_organization())) {
            Ok(a) => a,
            Err(e) => unreachable!("construct: {:?}", e),
        };
        assert!(managed.is_managed());

        // A pre-existing site account - root's, another team's, anything not
        // created by OpenPortal - must not be treated as ours.
        for foreign in ["", "root", "physics", "OpenPortal", "openportal2"] {
            let account = match SlurmAccount::construct(&account_json(foreign)) {
                Ok(a) => a,
                Err(e) => unreachable!("construct({:?}): {:?}", foreign, e),
            };
            assert!(
                !account.is_managed(),
                "account in organization {:?} must not be considered managed",
                foreign
            );
        }
    }

    #[test]
    fn test_api_version_parsing_tolerates_a_hostile_version_string() {
        // The version comes from the server's openapi.json and used to be
        // indexed at element 2 unconditionally, so a two-component version -
        // legitimate or hostile - aborted the process. See findings R1/R27.
        let ok = |s: &str| match parse_api_version(s) {
            Ok(v) => v,
            Err(e) => unreachable!("parse_api_version({:?}): {:?}", s, e),
        };

        assert_eq!(ok("dbv0.0.40"), ("0.0.40".to_string(), vec![0, 0, 40]));
        // the trailing '&something' form the server sometimes uses
        assert_eq!(
            ok("dbv0.0.40&openapi/slurmdbd"),
            ("0.0.40".to_string(), vec![0, 0, 40])
        );
        // two components: parses, and the caller must cope with no element 2
        assert_eq!(ok("dbv1.2"), ("1.2".to_string(), vec![1, 2]));
        assert!(ok("dbv1").1.get(2).is_none());

        // malformed input is an error, never a panic
        // (the leading tag is not itself validated - any "…v<numbers>" is
        // accepted - but a non-numeric component never is)
        for bad in [
            "dbv",
            "dbv1.x.3",
            "dbv..",
            "dbv-1.2.3",
            "dbv4294967296.0.0",
            "",
        ] {
            assert!(
                parse_api_version(bad).is_err(),
                "{:?} must be rejected, not parsed",
                bad
            );
        }
    }

    #[test]
    fn test_clean_account_name_rejects_empty_and_normalises_separators() {
        assert!(clean_account_name("").is_err());
        assert!(clean_account_name("   ").is_err());
        assert!(clean_user_name("").is_err());

        assert_eq!(
            match clean_account_name("  My/Project Name  ") {
                Ok(n) => n,
                Err(e) => unreachable!("clean: {:?}", e),
            },
            "my_project_name"
        );
    }

    #[test]
    fn test_encode_path_segment_blocks_url_injection() {
        // Regression test for finding R14. An account or user name is
        // interpolated into a slurmrestd *path*, and `Url::parse_with_params`
        // appends to whatever query the interpolated string introduces - so an
        // unencoded `?` became a real query parameter.
        assert_eq!(encode_path_segment("proj.portal"), "proj.portal");
        assert_eq!(encode_path_segment("a-b_c1~"), "a-b_c1~");

        assert_eq!(
            encode_path_segment("x?with_deleted=true"),
            "x%3Fwith_deleted%3Dtrue"
        );
        assert_eq!(encode_path_segment("a/b"), "a%2Fb");
        assert_eq!(encode_path_segment("a#frag"), "a%23frag");
        assert_eq!(encode_path_segment("a&b=c"), "a%26b%3Dc");
        assert_eq!(encode_path_segment("a b"), "a%20b");
        assert_eq!(encode_path_segment("100%"), "100%25");

        // Nothing outside the unreserved set survives unencoded. `%` is
        // excluded from this loop because it is the escape prefix itself - it
        // is covered by the `100%` case above, which asserts it becomes `%25`.
        for injected in ["?", "#", "/", "&", "=", " ", ";", ":", "@", "+"] {
            let encoded = encode_path_segment(&format!("name{}x", injected));
            assert!(
                !encoded.contains(injected),
                "{:?} must not survive encoding (got {:?})",
                injected,
                encoded
            );
        }
    }
}
