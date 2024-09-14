// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use anyhow::Result;
use anyhow::{Context, Error as AnyError};
use reqwest::{cookie::Jar, Client};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::sync::Arc;
use thiserror::Error;

#[derive(Debug, Clone, Default)]
pub struct IPAUser {
    uid: String,
    givenname: String,
    userclass: String,
}

impl IPAUser {
    fn construct(result: &serde_json::Value) -> Result<Vec<IPAUser>, Error> {
        let mut users = Vec::new();

        // convert result into an array if it isn't already
        let result = match result.as_array() {
            Some(result) => result.clone(),
            None => vec![result.clone()],
        };

        for user in result {
            tracing::info!("User: {:?}", user);

            // uid is a list of strings - just get the first one
            let uid = user
                .get("uid")
                .and_then(|v| v.as_array())
                .and_then(|v| v.first())
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();

            let givenname = user
                .get("givenname")
                .and_then(|v| v.as_array())
                .and_then(|v| v.first())
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();

            let userclass = user
                .get("userclass")
                .and_then(|v| v.as_array())
                .and_then(|v| v.first())
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();

            users.push(IPAUser {
                uid,
                givenname,
                userclass,
            });
        }

        Ok(users)
    }

    pub fn username(&self) -> &str {
        &self.uid
    }

    pub fn givenname(&self) -> &str {
        &self.givenname
    }

    pub fn userclass(&self) -> &str {
        &self.userclass
    }
}

#[derive(Debug, Clone, Default)]
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

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
#[serde(default)]
struct UserFindResponse {
    count: Option<u32>,
    messages: Option<serde_json::Value>,
    summary: Option<String>,
    result: Option<serde_json::Value>,
    truncated: Option<bool>,
}

impl UserFindResponse {
    fn users(&self) -> Result<Vec<IPAUser>, Error> {
        IPAUser::construct(&self.result.clone().unwrap_or_default())
    }
}

#[derive(Debug, Clone, Default)]
pub struct FreeIPA {
    auth: FreeAuth,
}

impl FreeIPA {
    pub async fn connect(server: &str, user: &str, password: &str) -> Result<Self, Error> {
        Ok(FreeIPA {
            auth: login(server, user, password).await?,
        })
    }

    pub async fn users_in_group(&self, group: &str) -> Result<Vec<IPAUser>, Error> {
        let result =
            call_post::<UserFindResponse>(&self.auth, "user_find", Some(vec![group.to_string()]))
                .await?;

        result.users()
    }

    pub async fn user(&self, user: &str) -> Result<Option<IPAUser>, Error> {
        let result =
            call_post::<UserFindResponse>(&self.auth, "user_show", Some(vec![user.to_string()]))
                .await?;

        Ok(result.users()?.first().cloned())
    }
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
