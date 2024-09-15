// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use anyhow::Result;
use anyhow::{Context, Error as AnyError};
use reqwest::{cookie::Jar, Client};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use thiserror::Error;

#[derive(Debug, Clone, Default)]
pub struct IPAUser {
    userid: String,
    givenname: String,
    homedirectory: String,
    userclass: String,
    memberof: Vec<String>,
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
            // uid is a list of strings - just get the first one
            let userid = user
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

            let homedirectory: String = user
                .get("homedirectory")
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

            let memberof = user
                .get("memberof_group")
                .and_then(|v| v.as_array())
                .map(|v| {
                    v.iter()
                        .filter_map(|v| v.as_str())
                        .map(|v| v.to_string())
                        .collect()
                })
                .unwrap_or_default();

            users.push(IPAUser {
                userid,
                givenname,
                userclass,
                homedirectory,
                memberof,
            });
        }

        Ok(users)
    }

    pub fn userid(&self) -> &str {
        &self.userid
    }

    pub fn givenname(&self) -> &str {
        &self.givenname
    }

    pub fn userclass(&self) -> &str {
        &self.userclass
    }

    pub fn homedirectory(&self) -> &str {
        &self.homedirectory
    }

    pub fn memberof(&self) -> &Vec<String> {
        &self.memberof
    }
}

#[derive(Debug, Clone, Default)]
pub struct IPAGroup {
    groupid: String,
    description: String,
}

impl IPAGroup {
    fn construct(result: &serde_json::Value) -> Result<Vec<IPAGroup>, Error> {
        let mut groups = Vec::new();

        // convert result into an array if it isn't already
        let result = match result.as_array() {
            Some(result) => result.clone(),
            None => vec![result.clone()],
        };

        for group in result {
            // uid is a list of strings - just get the first one
            let groupid = group
                .get("cn")
                .and_then(|v| v.as_array())
                .and_then(|v| v.first())
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();

            let description = group
                .get("description")
                .and_then(|v| v.as_array())
                .and_then(|v| v.first())
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();

            groups.push(IPAGroup {
                groupid,
                description,
            });
        }

        Ok(groups)
    }

    pub fn groupid(&self) -> &str {
        &self.groupid
    }

    pub fn description(&self) -> &str {
        &self.description
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
async fn call_post<T>(
    auth: &FreeAuth,
    func: &str,
    args: Option<Vec<String>>,
    kwargs: Option<HashMap<String, String>>,
) -> Result<T, Error>
where
    T: DeserializeOwned,
{
    let url = format!("{}/ipa/session/json", &auth.server);

    // make id a random integer between 1 and 1000
    let id = rand::random::<u16>() % 1000;

    let mut kwargs = kwargs.unwrap_or_default();
    kwargs.insert("version".to_string(), "2.251".to_string());

    // the payload is a json object that contains the method, the parameters
    // (as an array, plus a dict of the version) and a random id. The id
    // will be passed back to us in the response.
    let payload = serde_json::json!({
        "method": func,
        "params": [args.unwrap_or_default(), kwargs],
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
struct IPAResponse {
    count: Option<u32>,
    messages: Option<serde_json::Value>,
    summary: Option<String>,
    result: Option<serde_json::Value>,
    truncated: Option<bool>,
}

impl IPAResponse {
    fn users(&self) -> Result<Vec<IPAUser>, Error> {
        IPAUser::construct(&self.result.clone().unwrap_or_default())
    }

    fn groups(&self) -> Result<Vec<IPAGroup>, Error> {
        IPAGroup::construct(&self.result.clone().unwrap_or_default())
    }
}

#[derive(Debug, Clone, Default)]
pub struct FreeIPA {
    auth: FreeAuth,
    user: String,
    password: String,
}

impl FreeIPA {
    pub async fn connect(server: &str, user: &str, password: &str) -> Result<Self, Error> {
        Ok(FreeIPA {
            auth: login(server, user, password).await?,
            user: user.to_string(),
            password: password.to_string(),
        })
    }

    pub async fn reconnect(&mut self) -> Result<(), Error> {
        Ok(self.auth = login(&self.auth.server, &self.user, &self.password).await?)
    }

    pub async fn users(&self) -> Result<Vec<IPAUser>, Error> {
        let result = call_post::<IPAResponse>(&self.auth, "user_find", None, None).await?;

        result.users()
    }

    pub async fn groups(&self) -> Result<Vec<IPAGroup>, Error> {
        let result = call_post::<IPAResponse>(&self.auth, "group_find", None, None).await?;

        result.groups()
    }

    pub async fn users_in_group(&self, group: &str) -> Result<Vec<IPAUser>, Error> {
        // call the freeipa api to find users in the passed group
        let kwargs = {
            let mut kwargs = HashMap::new();
            kwargs.insert("in_group".to_string(), group.to_string());
            kwargs
        };

        let result = call_post::<IPAResponse>(&self.auth, "user_find", None, Some(kwargs)).await?;

        result.users()
    }

    pub async fn user(&self, user: &str) -> Result<Option<IPAUser>, Error> {
        let result =
            call_post::<IPAResponse>(&self.auth, "user_find", Some(vec![user.to_string()]), None)
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
