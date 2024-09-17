// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use anyhow::Context;
use anyhow::Result;
use once_cell::sync::Lazy;
use reqwest::{cookie::Jar, Client};
use secrecy::ExposeSecret;
use secrecy::Secret;
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use templemeads::grammar::UserIdentifier;
use templemeads::Error;
use tokio::sync::{Mutex, MutexGuard};

use crate::db;

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
    func: &str,
    args: Option<Vec<String>>,
    kwargs: Option<HashMap<String, String>>,
) -> Result<T, Error>
where
    T: DeserializeOwned,
{
    // get the auth details from the global FreeIPA client
    let mut auth = auth().await?;
    auth.num_reconnects = 0;

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
        "params": [args.clone().unwrap_or_default(), kwargs.clone()],
        "id": id,
    });

    let client = Client::builder()
        .cookie_provider(Arc::clone(&auth.jar))
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

        match login(&auth.server, &auth.user, &auth.password).await {
            Ok(jar) => {
                auth.jar = jar;

                // create a new client with the new cookies
                let client = Client::builder()
                    .cookie_provider(Arc::clone(&auth.jar))
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

#[derive(Debug, Clone)]
struct FreeAuth {
    server: String,
    jar: Arc<Jar>,
    user: String,
    password: Secret<String>,
    num_reconnects: u32,
}

impl FreeAuth {
    fn default() -> Self {
        FreeAuth {
            server: "".to_string(),
            jar: Arc::new(Jar::default()),
            user: "".to_string(),
            password: Secret::new("".to_string()),
            num_reconnects: 0,
        }
    }
}

static FREEIPA_AUTH: Lazy<Mutex<FreeAuth>> = Lazy::new(|| Mutex::new(FreeAuth::default()));

///
/// Login to the FreeIPA server using the passed username and password.
/// This returns a cookie jar that will contain the resulting authorisation
/// cookie, and which can be used for subsequent calls to the server.
///
async fn login(server: &str, user: &str, password: &Secret<String>) -> Result<Arc<Jar>, Error> {
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
        .body(format!(
            "user={}&password={}",
            user,
            password.expose_secret()
        ))
        .send()
        .await
        .with_context(|| format!("Could not login calling URL: {}", url))?;

    match result.status() {
        status if status.is_success() => Ok(jar),
        _ => Err(Error::Login(format!(
            "Could not login to server: {}. Status: {}. Response: {:?}",
            server,
            result.status(),
            result
        ))),
    }
}

// function to return the client protected by a MutexGuard
async fn auth<'mg>() -> Result<MutexGuard<'mg, FreeAuth>, Error> {
    Ok(FREEIPA_AUTH.lock().await)
}

///
/// Public API
///

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

    pub fn identifier(&self) -> UserIdentifier {
        // we have put the OpenPortal UserIdentifier into the
        // "givenname" field of the user
        UserIdentifier::new(&self.userid)
    }
}

#[derive(Debug, Clone, Default)]
pub struct IPAGroup {
    groupid: String,
    description: String,
}

impl IPAGroup {
    fn new(groupid: &str, description: &str) -> Result<Self, Error> {
        // check that the groupid is valid .... PARSING RULES

        Ok(IPAGroup {
            groupid: groupid.to_string(),
            description: description.to_string(),
        })
    }

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

    pub fn parse(groups: &str) -> Result<Vec<IPAGroup>, Error> {
        let mut g = Vec::new();
        let mut errors = Vec::new();

        for group in groups.split(",") {
            if !group.is_empty() {
                match IPAGroup::new(group, "OpenPortal-managed group") {
                    Ok(group) => g.push(group),
                    Err(_) => errors.push(group),
                }
            }
        }

        if !errors.is_empty() {
            return Err(Error::Parse(format!(
                "Could not parse groups: {:?}",
                errors.join(",")
            )));
        }

        Ok(g)
    }

    pub fn groupid(&self) -> &str {
        &self.groupid
    }

    pub fn description(&self) -> &str {
        &self.description
    }
}

pub async fn connect(server: &str, user: &str, password: &Secret<String>) -> Result<(), Error> {
    // overwrite the global FreeIPA client with a new one
    let mut auth = FREEIPA_AUTH.lock().await;

    auth.server = server.to_string();
    auth.user = user.to_string();
    auth.password = password.clone();
    auth.num_reconnects = 0;

    const MAX_RECONNECTS: u32 = 3;
    const RECONNECT_WAIT: u64 = 100;

    loop {
        match login(&auth.server, &auth.user, &auth.password).await {
            Ok(jar) => {
                auth.jar = jar;
                return Ok(());
            }
            Err(e) => {
                tracing::error!(
                    "Could not login to FreeIPA server: {}. Error: {}",
                    server,
                    e
                );

                auth.num_reconnects += 1;

                if auth.num_reconnects > MAX_RECONNECTS {
                    return Err(Error::Login(format!(
                        "Could not login to FreeIPA server: {}. Error: {}",
                        server, e
                    )));
                }

                tokio::time::sleep(std::time::Duration::from_millis(RECONNECT_WAIT)).await;
            }
        }
    }
}

pub async fn get_users() -> Result<Vec<IPAUser>, Error> {
    let result = call_post::<IPAResponse>("user_find", None, None).await?;
    result.users()
}

pub async fn get_group(group: &str) -> Result<Option<IPAGroup>, Error> {
    let result =
        call_post::<IPAResponse>("group_find", Some(vec![group.to_string()]), None).await?;

    Ok(result.groups()?.first().cloned())
}

pub async fn get_groups() -> Result<Vec<IPAGroup>, Error> {
    let result = call_post::<IPAResponse>("group_find", None, None).await?;
    result.groups()
}

pub async fn get_users_in_group(group: &str) -> Result<Vec<IPAUser>, Error> {
    // call the freeipa api to find users in the passed group
    let kwargs = {
        let mut kwargs = HashMap::new();
        kwargs.insert("in_group".to_string(), group.to_string());
        kwargs
    };

    let result = call_post::<IPAResponse>("user_find", None, Some(kwargs)).await?;
    result.users()
}

pub async fn get_user(user: &str) -> Result<Option<IPAUser>, Error> {
    let result = call_post::<IPAResponse>("user_find", Some(vec![user.to_string()]), None).await?;

    Ok(result.users()?.first().cloned())
}

pub async fn add_user(user: &UserIdentifier) -> Result<IPAUser, Error> {
    // check to see if the user already exists
    if let Some(user) = db::get_user(user).await? {
        return Ok(user);
    }

    // the user probably doesn't exist, so add them, making sure they
    // are in the correct groups
    let mut groups = db::get_system_groups().await?;

    // add in the "openportal" group, to which all users managed by
    // OpenPortal must belong
    groups.push(IPAGroup::new(
        "openportal",
        "Group for all users managed by OpenPortal",
    )?);

    // also add in the group for the user's project
    groups.push(IPAGroup::new(
        &format!("project.{}", user.project()),
        &format!("Group for all users in the {} project", user.project()),
    )?);

    tracing::info!("Adding user: {:?} to groups {:?}", user, groups);

    // first step, make sure that all of the groups exist - and get their CNs
    let mut group_cns = Vec::new();

    for group in &groups {
        // check if it exists
        match get_group(group.groupid()).await? {
            None => {
                // create the group
                let kwargs = {
                    let mut kwargs = HashMap::new();
                    kwargs.insert("cn".to_string(), group.groupid().to_string());
                    kwargs.insert("description".to_string(), group.description().to_string());
                    kwargs
                };

                match call_post::<IPAResponse>("group_add", None, Some(kwargs)).await {
                    Ok(_) => {
                        tracing::info!("Successfully created group: {:?}", group);
                        group_cns.push(group.groupid().to_string());
                    }
                    Err(e) => {
                        tracing::error!("Could not add group: {:?}. Error: {}", group, e);
                    }
                }
            }
            Some(group) => {
                group_cns.push(group.groupid().to_string());
            }
        }
    }

    // now add the user
    let kwargs = {
        let mut kwargs = HashMap::new();
        kwargs.insert(
            "uid".to_string(),
            format!("{}.{}", user.username(), user.project()),
        );
        kwargs.insert("givenname".to_string(), user.username().to_string());
        kwargs.insert("sn".to_string(), user.project().to_string());
        kwargs.insert("userclass".to_string(), "openportal".to_string());
        kwargs.insert("cn".to_string(), user.to_string());

        kwargs
    };

    let result = call_post::<IPAResponse>("user_add", None, Some(kwargs)).await?;
    let user = result.users()?.first().cloned().ok_or(Error::Call(format!(
        "User {:?} could not be found after adding?",
        user
    )))?;

    let userid = user.userid().to_string();

    // now add the user to all of the groups
    for group_cn in &group_cns {
        let kwargs = {
            let mut kwargs = HashMap::new();
            kwargs.insert("cn".to_string(), group_cn.clone());
            kwargs.insert("user".to_string(), userid.clone());
            kwargs
        };

        match call_post::<IPAResponse>("group_add_member", None, Some(kwargs)).await {
            Ok(_) => tracing::info!(
                "Successfully added user {:?} to group {:?}",
                userid,
                group_cn
            ),
            Err(e) => {
                tracing::error!(
                    "Could not add user {:?} to group {:?}. Error: {}",
                    userid,
                    group_cn,
                    e
                );
            }
        }
    }

    // finally - re-fetch the user from FreeIPA to make sure that we have
    // the correct information
    let user = get_user(user.userid()).await?.ok_or(Error::Call(format!(
        "User {:?} could not be found after adding?",
        user
    )))?;

    tracing::info!("User {:?} added", user);

    Ok(user)
}
