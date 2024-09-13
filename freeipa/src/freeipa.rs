// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use anyhow::Result;
use anyhow::{Context, Error as AnyError};
use reqwest::{cookie::Jar, Client};
use serde::ser;
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::sync::Arc;
use thiserror::Error;

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default)]
struct IPAUserForDeserialize {
    uid: Vec<String>,
    givenname: Vec<String>,
    userclass: Vec<String>,
    version: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(default)]
struct IPAUser {
    uid: String,
    givenname: String,
    userclass: String,
}

impl From<IPAUserForDeserialize> for IPAUser {
    fn from(user: IPAUserForDeserialize) -> Self {
        IPAUser {
            // extract the first item or use an empty string
            uid: user.uid.first().unwrap_or(&"".to_string()).clone(),
            givenname: user.givenname.first().unwrap_or(&"".to_string()).clone(),
            userclass: user.userclass.first().unwrap_or(&"".to_string()).clone(),
        }
    }
}

fn deserialize_users<'de, D>(deserializer: D) -> Result<Vec<IPAUser>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let users: Vec<IPAUserForDeserialize> = Vec::deserialize(deserializer)?;

    Ok(users.into_iter().map(|u| u.into()).collect())
}

struct FreeAuth {
    server: String,
    jar: Arc<Jar>,
}

///
/// Login to the FreeIPA server using the passed username and password.
/// This returns a cookie jar that will contain the resulting authorisation
/// cookie, and which can be used for subsequent calls to the server.
///
async fn login(server: &str, user: &str, password: &str) -> Result<FreeAuth, Error> {
    let jar = Arc::new(Jar::default());

    let client = Client::builder()
        .cookie_provider(Arc::clone(&jar))
        .build()
        .context("Could not build client")?;

    let url = format!("{}/ipa/session/login_password", server);

    let result = client
        .post(&url)
        .header("Referer", format!("{}/ipa", server))
        .header("Content-Type", "application/x-www-form-urlencoded")
        .header("Accept", "text/plain")
        .body(format!("user={}&password={}", user, password))
        .send()
        .await
        .with_context(|| format!("Could not login calling URL: {}", url))?;

    match result.status() {
        status if status.is_success() => Ok(FreeAuth {
            server: server.to_string(),
            jar,
        }),
        _ => Err(Error::Login(format!(
            "Could not login to server: {}. Status: {}. Response: {:?}",
            server,
            result.status(),
            result
        ))),
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct FreeResponse {
    result: serde_json::Value,
    principal: serde_json::Value,
    error: serde_json::Value,
    id: u16,
}

///
/// Call a post URL on the FreeIPA server described in 'auth'.
///
async fn call_post<T>(auth: &FreeAuth, func: &str, params: Option<Vec<String>>) -> Result<T, Error>
where
    T: DeserializeOwned,
{
    let url = format!("{}/ipa/session/json", &auth.server);

    // make id a random integer between 1 and 1000
    let id = rand::random::<u16>() % 1000;

    // the payload is a json object that contains the method, the parameters
    // (as an array, plus a dict of the version) and a random id. The id
    // will be passed back to us in the response.
    let payload = serde_json::json!({
        "method": func,
        "params": [params.unwrap_or_default(), {"version": "2.251"}],
        "id": id,
    });

    let client = Client::builder()
        .cookie_provider(Arc::clone(&auth.jar))
        .build()
        .context("Could not build client")?;

    let result = client
        .post(&url)
        .header("Referer", format!("{}/ipa", &auth.server))
        .header("Content-Type", "application/json")
        .header("Accept", "application/json")
        .json(&payload)
        .send()
        .await
        .with_context(|| format!("Could not call function: {}", payload))?;

    if result.status().is_success() {
        let result = result
            .json::<FreeResponse>()
            .await
            .context("Could not decode from json")?;

        // assert that the id numbers match
        if result.id != id {
            return Err(Error::Call(format!(
                "ID mismatch: expected {}, got {}",
                id, result.id
            )));
        }

        // if there is an error, return it
        if !result.error.is_null() {
            return Err(Error::Call(format!(
                "Error in response: {:?}",
                result.error
            )));
        }

        // return the result, encoded to the type T
        Ok(serde_json::from_value(result.result).context("Could not decode result")?)
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

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
#[serde(default)]
struct UserFindResponse {
    count: u32,
    messages: serde_json::Value,
    summary: String,
    /// have serde deserialize this as a Vec<IPAUserForDeserialize> and then
    /// convert it to a Vec<IPAUser>
    #[serde(deserialize_with = "deserialize_users")]
    result: Vec<IPAUser>,
    truncated: bool,
}

pub async fn connect() -> Result<(), Error> {
    let auth = login("https://ipa.demo1.freeipa.org", "admin", "Secret123").await?;

    let result = call_post::<UserFindResponse>(&auth, "user_find", None).await?;

    tracing::info!("Result: {:?}", result);

    Ok(())
}

/// Errors

#[derive(Error, Debug)]
pub enum Error {
    #[error("{0}")]
    Any(#[from] AnyError),

    #[error("{0}")]
    Call(String),

    #[error("{0}")]
    Login(String),
}
