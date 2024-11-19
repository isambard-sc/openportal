// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use anyhow::Context;
use anyhow::Result;
use once_cell::sync::Lazy;
use reqwest::Client;
use secrecy::{ExposeSecret, Secret};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::fmt::Display;
use templemeads::grammar::UserMapping;
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
async fn call_get(backend: &str, function: &str) -> Result<serde_json::Value, Error> {
    // get the auth details from the global Slurm client
    let mut auth = auth().await?;
    auth.num_reconnects = 0;

    let url = format!(
        "{}/{}/v{}/{}",
        &auth.server, backend, &auth.version, function
    );

    tracing::info!("Calling function {}", url);

    let client = Client::builder()
        .build()
        .context("Could not build client")?;

    let mut result = client
        .get(&url)
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

        match login(&auth.server, &auth.user, &auth.token_command).await {
            Ok(session) => {
                auth.jwt = session.jwt;
                auth.version = session.version;

                // create a new client with the new cookies
                let client = Client::builder()
                    .build()
                    .context("Could not build client")?;

                // retry the call
                result = client
                    .get(&url)
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

        match login(&auth.server, &auth.user, &auth.token_command).await {
            Ok(session) => {
                auth.jwt = session.jwt;
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
    user: String,
    jwt: Secret<String>,
    version: String,
    num_reconnects: u32,
}

impl SlurmAuth {
    fn default() -> Self {
        SlurmAuth {
            server: "".to_string(),
            token_command: "".to_string(),
            user: "".to_string(),
            jwt: Secret::new("".to_string()),
            version: "".to_string(),
            num_reconnects: 0,
        }
    }
}

static SLURM_AUTH: Lazy<Mutex<SlurmAuth>> = Lazy::new(|| Mutex::new(SlurmAuth::default()));

struct SlurmSession {
    jwt: Secret<String>,
    version: String,
}

///
/// Login to the Slurm server using the passed passed command to generate
/// the JWT token. This will return the valid JWT in a secret. This
/// JWT can be used for subsequent calls to the server.
///
async fn login(server: &str, user: &str, token_command: &str) -> Result<SlurmSession, Error> {
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

    tracing::info!("Getting JWT token from command: {}", token_command);

    // parse 'token_command' into an executable plus arguments
    let token_command = shlex::split(&token_command).context("Could not parse token command")?;

    let token_exe = token_command.first().context("No token command")?;
    let token_args = token_command.get(1..).unwrap_or(&[]);

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
        .context("Could not split version")?;

    tracing::info!("Extracted version: {}", version);

    // now call the ping function to make sure that the server is
    // up and running
    let url = format!("{}/slurm/v{}/ping", server, version);

    let result = client
        .get(&url)
        .header("Accept", "application/json")
        .header("X-SLURM-USER-NAME", user)
        .header("X-SLURM-USER-TOKEN", jwt.clone())
        .send()
        .await
        .with_context(|| format!("Could not ping slurm at URL: {}", url))?;

    // convert the response to JSON
    let ping_response = match &result.json::<serde_json::Value>().await {
        Ok(json) => json.clone(),
        Err(e) => {
            tracing::error!("Could not decode JSON from ping response: {}", e);
            return Err(Error::Login(format!(
                "Could not decode JSON from ping response: {}",
                e
            )));
        }
    };

    tracing::info!("Ping response: {:?}", ping_response);

    Ok(SlurmSession {
        jwt: Secret::new(jwt),
        version: version.to_string(),
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
    //            organization: "default"
    //        }
    //    ]
    // }
    // (we always use the default organization)

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

///
/// Public API
///

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct SlurmAccount {
    name: String,
    description: String,
    organization: String,
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
    pub fn from_mapping(mapping: &UserMapping) -> Self {
        SlurmAccount {
            name: mapping.local_project().to_string(),
            description: format!(
                "Account for OpenPortal project {}",
                mapping.user().project()
            ),
            organization: "default".to_string(),
        }
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct SlurmUser {
    name: String,
}

impl Display for SlurmUser {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "SlurmUser {{ name: {} }}", self.name())
    }
}

impl SlurmUser {
    pub fn from_mapping(mapping: &UserMapping) -> Self {
        SlurmUser {
            name: mapping.local_user().to_string(),
        }
    }

    pub fn name(&self) -> &str {
        &self.name
    }
}

pub async fn connect(server: &str, user: &str, token_command: &str) -> Result<(), Error> {
    // overwrite the global FreeIPA client with a new one
    let mut auth = auth().await?;

    auth.server = server.to_string();
    auth.user = user.to_string();
    auth.token_command = token_command.to_string();
    auth.jwt = Secret::new("".to_string());
    auth.num_reconnects = 0;

    const MAX_RECONNECTS: u32 = 3;
    const RECONNECT_WAIT: u64 = 100;

    loop {
        match login(&auth.server, &auth.user, &auth.token_command).await {
            Ok(session) => {
                auth.jwt = session.jwt;
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

async fn get_slurm_account_from_slurm(account: &str) -> Result<Option<SlurmAccount>, Error> {
    let response = match call_get("slurmdb", &format!("account/{}", account)).await {
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
        name == Some(account)
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

    // response should have a name, description and organization
    let name = match account.get("name") {
        Some(name) => name,
        None => {
            tracing::warn!("Could not get name from account: {:?}", account);
            return Ok(None);
        }
    };

    let name = match name.as_str() {
        Some(name) => name,
        None => {
            tracing::warn!("Could not get name as string from account: {:?}", name);
            return Ok(None);
        }
    };

    let description = match account.get("description") {
        Some(description) => description,
        None => {
            tracing::warn!("Could not get description from account: {:?}", account);
            return Ok(None);
        }
    };

    let description = match description.as_str() {
        Some(description) => description,
        None => {
            tracing::warn!(
                "Could not get description as string from account: {:?}",
                description
            );
            return Ok(None);
        }
    };

    let organization = match account.get("organization") {
        Some(organization) => organization,
        None => {
            tracing::warn!("Could not get organization from account: {:?}", account);
            return Ok(None);
        }
    };

    let organization = match organization.as_str() {
        Some(organization) => organization,
        None => {
            tracing::warn!(
                "Could not get organization as string from account: {:?}",
                organization
            );
            return Ok(None);
        }
    };

    Ok(Some(SlurmAccount {
        name: name.to_string(),
        description: description.to_string(),
        organization: organization.to_string(),
    }))
}

async fn get_slurm_account(account: &str) -> Result<Option<SlurmAccount>, Error> {
    // need to GET /slurm/vX.Y.Z/accounts/{account.name}
    // and return the account if it exists
    let account = cache::get_account(account).await?;

    if let Some(account) = account {
        // double-check that the account actually exists...
        let existing_account = match get_slurm_account_from_slurm(account.name()).await {
            Ok(account) => account,
            Err(e) => {
                tracing::warn!("Could not get account {}: {}", account.name(), e);
                cache::clear().await?;
                return Ok(None);
            }
        };

        if let Some(existing_account) = existing_account {
            if account != existing_account {
                tracing::warn!(
                    "Account {} exists, but with different details.",
                    account.name()
                );
                tracing::warn!("Existing: {:?}, new: {:?}", existing_account, account);

                // clear the cache as something has changed behind our back
                cache::clear().await?;

                // store the new account
                cache::add_account(&existing_account).await?;

                return Ok(Some(existing_account));
            } else {
                return Ok(Some(account));
            }
        } else {
            // the account doesn't exist
            tracing::warn!(
                "Account {} does not exist - it has been removed from slurm.",
                account.name()
            );
            cache::clear().await?;
            return Ok(None);
        }
    }

    // the account doesn't exist
    Ok(None)
}

async fn get_slurm_account_create_if_not_exists(
    account: &SlurmAccount,
) -> Result<SlurmAccount, Error> {
    let existing_account = get_slurm_account(account.name()).await?;

    if let Some(existing_account) = existing_account {
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

        return Ok(existing_account);
    }

    // it doesn't, so create it
    let account = force_add_slurm_account(account).await?;
    cache::add_account(&account).await?;

    Ok(account.clone())
}

async fn associate_slurm_user(
    user: &SlurmUser,
    account: &SlurmAccount,
) -> Result<SlurmUser, Error> {
    // need to POST to /slurm/vX.Y.Z/accounts/{account.name}/users
    // with a JSON payload
    // {
    //    users: [
    //        {
    //            name: "user"
    //        }
    //    ]
    // }

    let payload = serde_json::json!({
        "users": [
            {
                "name": user.name
            }
        ]
    });

    call_post(
        "slurmdb",
        &format!("account/{}/users", account.name),
        &payload,
    )
    .await?;

    Ok(user.clone())
}

pub async fn add_user(user: &UserMapping) -> Result<(), Error> {
    let account = get_slurm_account_create_if_not_exists(&SlurmAccount::from_mapping(user)).await?;

    let test_account = get_slurm_account(account.name()).await?;

    tracing::info!("Test account: {:?}", test_account);

    let user = associate_slurm_user(&SlurmUser::from_mapping(user), &account).await?;

    tracing::info!("Associated user {} with account {}", user, account);

    Ok(())
}
