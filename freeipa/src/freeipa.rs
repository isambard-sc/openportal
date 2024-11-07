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
use templemeads::grammar::{UserIdentifier, UserMapping};
use templemeads::Error;
use tokio::sync::{Mutex, MutexGuard};

use crate::cache;

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
    cn: UserIdentifier,
    givenname: String,
    homedirectory: String,
    userclass: String,
    primary_group: String,
    memberof: Vec<String>,
}

// implement display for IPAUser
impl std::fmt::Display for IPAUser {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}: local_name={}, givenname={}, primary_group={}, memberof={}, home={}",
            self.identifier(),
            self.userid(),
            self.givenname(),
            self.primary_group(),
            self.memberof().join(","),
            self.home(),
        )
    }
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
            let cn = match UserIdentifier::parse(
                user.get("cn")
                    .and_then(|v| v.as_array())
                    .and_then(|v| v.first())
                    .and_then(|v| v.as_str())
                    .unwrap_or_default(),
            ) {
                Ok(cn) => cn,
                Err(e) => {
                    tracing::error!("Could not parse user identifier: {}. Error: {}", user, e);
                    continue;
                }
            };

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

            let memberof: Vec<String> = user
                .get("memberof_group")
                .and_then(|v| v.as_array())
                .map(|v| {
                    v.iter()
                        .filter_map(|v| v.as_str())
                        .map(|v| v.to_string())
                        .collect()
                })
                .unwrap_or_default();

            // try to find the primary group for this user
            let primary_group = get_primary_group(&cn);
            let primary_group = match memberof.contains(&primary_group) {
                true => primary_group,
                false => {
                    tracing::warn!("Could not find primary group for user: {}", cn);
                    "".to_string()
                }
            };

            users.push(IPAUser {
                userid,
                cn,
                givenname,
                userclass,
                homedirectory,
                primary_group,
                memberof,
            });
        }

        Ok(users)
    }

    ///
    /// Return the local user identifier (local unix account)
    /// for this user
    ///
    pub fn userid(&self) -> &str {
        self.local_username()
    }

    ///
    /// Return the givenname for this user (this is the full user.project.portal)
    ///
    pub fn givenname(&self) -> &str {
        &self.givenname
    }

    ///
    /// Return the userclass for this user - it should be "openportal"
    ///
    pub fn userclass(&self) -> &str {
        &self.userclass
    }

    ///
    /// Return the home directory for this user
    ///
    pub fn home(&self) -> &str {
        &self.homedirectory
    }

    ///
    /// Return the primary group for this user - this should be
    /// the project group
    ///
    pub fn primary_group(&self) -> &str {
        &self.primary_group
    }

    ///
    /// Return the groups that this user is a member of
    ///
    pub fn memberof(&self) -> &Vec<String> {
        &self.memberof
    }

    ///
    /// Return the UserIdentifier for this user (user.project.portal)
    ///
    pub fn identifier(&self) -> &UserIdentifier {
        // we have put the OpenPortal UserIdentifier into the
        // "cn" field of the user
        &self.cn
    }

    ///
    /// Return the mapping from the UserIdentifier to the
    /// FreeIPA (local user account plus primary project) user
    ///
    pub fn mapping(&self) -> Result<UserMapping, Error> {
        UserMapping::new(&self.cn, self.userid(), self.primary_group())
    }

    ///
    /// Return the local username for this user (Unix account)
    ///
    pub fn local_username(&self) -> &str {
        // this is the linux user account
        &self.userid
    }

    ///
    /// Return whether this user is in the passed group
    ///
    pub fn in_group(&self, group: &str) -> bool {
        self.memberof().contains(&group.to_string())
    }

    ///
    /// Return whether or not this user is managed - only users
    /// in the "openportal" group can be managed
    ///
    pub fn is_managed(&self) -> bool {
        let managed_group = match get_managed_group() {
            Ok(group) => group.identifier().to_string(),
            Err(_) => return false,
        };

        self.in_group(&managed_group) && self.userclass() == managed_group
    }
}

#[derive(Debug, Clone, Default)]
pub struct IPAGroup {
    groupid: String,
    description: String,
}

// implement display for IPAGroup
impl std::fmt::Display for IPAGroup {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: description={}", self.groupid(), self.description())
    }
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

        for group in groups.split(',') {
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

    pub fn identifier(&self) -> &str {
        &self.groupid
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

///
/// Return the specified group from FreeIPA, or None if it does
/// not exist
///
pub async fn get_group(group: &str) -> Result<Option<IPAGroup>, Error> {
    match cache::get_group(group).await? {
        Some(group) => Ok(Some(group)),
        None => {
            let kwargs = {
                let mut kwargs = HashMap::new();
                kwargs.insert("cn".to_string(), group.to_string());
                kwargs
            };

            let result = call_post::<IPAResponse>("group_find", None, Some(kwargs)).await?;

            match result.groups()?.first() {
                Some(group) => {
                    cache::add_existing_group(group).await?;
                    Ok(Some(group.clone()))
                }
                None => Ok(None),
            }
        }
    }
}

///
/// Return the Unix username associated with the passed UserIdentifier.
///
/// Eventually we will need to deal with federation, and work
/// out a way to uniquely convert user identifiers to Unix usernames.
/// Currently, for the identifier user.group.portal, we require that
/// a portal ensures user.group is unique within the portal. For now,
/// we will just use user.group (as brics is the only portal)
///
async fn identifier_to_userid(user: &UserIdentifier) -> Result<String, Error> {
    Ok(format!("{}.{}", user.username(), user.project()))
}

///
/// Force get the user - this will refresh the data from FreeIPA
///
async fn force_get_user(user: &UserIdentifier) -> Result<Option<IPAUser>, Error> {
    let kwargs = {
        let mut kwargs = HashMap::new();
        kwargs.insert("all".to_string(), "true".to_string());
        kwargs.insert("cn".to_string(), user.to_string());
        kwargs.insert("uid".to_string(), identifier_to_userid(user).await?);
        kwargs
    };

    let result = call_post::<IPAResponse>("user_find", None, Some(kwargs)).await?;

    // this isn't one line because we need to specify the
    // type of 'users'
    let users: Vec<IPAUser> = result
        .users()?
        .into_iter()
        .filter(|user| user.is_managed())
        .collect();

    match users.first() {
        Some(user) => {
            cache::add_existing_user(user).await?;
            Ok(Some(user.clone()))
        }
        None => Ok(None),
    }
}

///
/// Return the user matching the passed identifier - note that
/// this will only return users who are managed (part of the
/// "openportal" group)
///
pub async fn get_user(user: &UserIdentifier) -> Result<Option<IPAUser>, Error> {
    match cache::get_user(user).await? {
        Some(user) => Ok(Some(user.clone())),
        None => Ok(force_get_user(user).await?),
    }
}

///
/// Call this function to get the group - adding it to FreeIPA if
/// it doesn't already exist
///
async fn get_group_create_if_not_exists(group: &IPAGroup) -> Result<IPAGroup, Error> {
    // check if it already exist in FreeIPA (this also checks cache)
    if let Some(group) = get_group(group.identifier()).await? {
        cache::add_existing_group(&group).await?;
        return Ok(group);
    }

    // it doesn't - try to create the group
    let kwargs = {
        let mut kwargs = HashMap::new();
        kwargs.insert("cn".to_string(), group.groupid().to_string());
        kwargs.insert("description".to_string(), group.description().to_string());
        kwargs
    };

    match call_post::<IPAResponse>("group_add", None, Some(kwargs)).await {
        Ok(_) => {
            tracing::info!("Successfully created group: {}", group);
        }
        Err(e) => {
            tracing::error!("Could not add group: {}. Error: {}", group, e);
        }
    }

    // the group should now exist in FreeIPA (either we added it,
    // or another thread beat us to it - get the group as it is in
    // FreeIPA
    match get_group(group.identifier()).await? {
        Some(group) => Ok(group),
        None => {
            tracing::error!("Failed to add group {} to FreeIPA", group);
            Err(Error::Call(format!(
                "Failed to add group {} to FreeIPA",
                group
            )))
        }
    }
}

///
/// Return the group that indicates that this user is managed
///
fn get_managed_group() -> Result<IPAGroup, Error> {
    IPAGroup::new("openportal", "Group for all users managed by OpenPortal")
}

///
/// Return the name of the primary group for the user
///
fn get_primary_group(user: &UserIdentifier) -> String {
    format!("{}.{}", user.portal(), user.project())
}

///
/// Call this function to synchronise the groups for the passed user.
/// This checks that the user is in the correct groups, and adds them
/// or removes them as necessary. Groups will match the project group,
/// the system groups, and the openportal group.
///
async fn sync_groups(user: &IPAUser) -> Result<IPAUser, Error> {
    // the user probably doesn't exist, so add them, making sure they
    // are in the correct groups
    let mut groups = cache::get_system_groups().await?;

    // add in the "openportal" group, to which all users managed by
    // OpenPortal must belong
    groups.push(get_managed_group()?);

    // also add in the group for the user's project (this is their primary group)
    let primary_group = get_primary_group(user.identifier());

    groups.push(IPAGroup::new(
        &primary_group,
        &format!(
            "Primary group for all users in the {} project",
            user.identifier().project()
        ),
    )?);

    // first step, make sure that all of the groups exist - and get their CNs
    let mut group_cns = Vec::new();

    for group in &groups {
        let added_group = get_group_create_if_not_exists(group).await?;

        if group.identifier() != added_group.identifier() {
            tracing::error!(
                "Disagreement of identifier of added group: {} versus {}",
                group,
                added_group
            );

            return Err(Error::InvalidState(format!(
                "Disagreement of identifier of added group: {} versus {}",
                group, added_group
            )));
        }

        group_cns.push(group.identifier().to_string());
    }

    // return the user in the system - check that the groups match
    let user = get_user(user.identifier())
        .await?
        .ok_or(Error::Call(format!(
            "User {} could not be found after adding?",
            user
        )))?;

    // We cannot do anything to a user who isn't managed
    if !user.is_managed() {
        tracing::error!(
            "Cannot sync groups for user {} as they are not managed by OpenPortal.",
            user.userid()
        );

        return Err(Error::UnmanagedUser(format!(
            "Cannot sync groups for user {} as they are not managed by OpenPortal.",
            user.userid()
        )));
    }

    // which groups are missing?
    group_cns.retain(|group| !user.in_group(group));

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
            Ok(_) => tracing::info!("Successfully added user {} to group {}", userid, group_cn),
            Err(e) => {
                // this should not happen - it indicates that the group has disappeared
                // since we last updated. Our cache is now likely out of date.
                tracing::error!(
                    "Could not add user {} to group {}. Error: {}",
                    userid,
                    group_cn,
                    e
                );
                tracing::info!("Clearing the cache as FreeIPA has changed behind our back.");
                cache::clear().await?;
                // Return None so that the caller handles this failure case
                return Err(Error::InvalidState(format!(
                    "Could not add user {} to group {}. Error: {}. Likely freeipa was changed behind our back!",
                    userid, group_cn, e
                )));
            }
        }
    }

    // finally - re-fetch the user from FreeIPA to make sure that we have
    // the correct information
    match force_get_user(user.identifier()).await? {
        Some(user) => Ok(user),
        None => {
            tracing::warn!(
                "Failed to sync groups for user {} as this user no longer exists in FreeIPA.",
                user.identifier()
            );
            tracing::info!("Clearing the cache as FreeIPA has changed behind our back.");
            cache::clear().await?;
            // Return None so that the caller handles this failure case
            Err(Error::InvalidState(format!(
                "Failed to sync groups for user {} as this user no longer exists in FreeIPA. Likely freeipa was changed behind our back!",
                user.identifier()
            )))
        }
    }
}

pub async fn add_user(user: &UserIdentifier) -> Result<IPAUser, Error> {
    // return the user if they already exist
    if let Some(user) = get_user(user).await? {
        // make sure that the groups are correct
        match sync_groups(&user).await {
            Ok(user) => {
                tracing::info!("Added user [cached] {}", user);
                return Ok(user);
            }
            Err(e) => {
                tracing::warn!(
                    "Failed to sync groups for user {} after adding. Error: {}",
                    user.identifier(),
                    e
                );
                tracing::info!(
                    "Will try to add user {} again, as the groups are not correct.",
                    user.identifier()
                );
            }
        }

        // we get here if the user has been removed from FreeIPA behind
        // our back - if this was the case, then the cache has been cleared
    }

    let managed_group = get_managed_group()?;

    // They don't exist, so try to add
    let kwargs = {
        let mut kwargs = HashMap::new();
        kwargs.insert("uid".to_string(), identifier_to_userid(user).await?);
        kwargs.insert("givenname".to_string(), user.username().to_string());
        kwargs.insert("sn".to_string(), user.project().to_string());
        kwargs.insert(
            "userclass".to_string(),
            managed_group.identifier().to_string(),
        );
        kwargs.insert("cn".to_string(), user.to_string());

        kwargs
    };

    let result = call_post::<IPAResponse>("user_add", None, Some(kwargs)).await?;
    let user = result.users()?.first().cloned().ok_or(Error::Call(format!(
        "User {} could not be found after adding?",
        user
    )))?;

    // add this user to the managed group so that it can be managed
    let userid = user.userid().to_string();

    // make sure that this group exists
    let managed_group = get_group_create_if_not_exists(&managed_group).await?;

    let kwargs = {
        let mut kwargs = HashMap::new();
        kwargs.insert("cn".to_string(), managed_group.groupid().to_string());
        kwargs.insert("user".to_string(), userid.clone());
        kwargs
    };

    match call_post::<IPAResponse>("group_add_member", None, Some(kwargs)).await {
        Ok(_) => tracing::info!(
            "Successfully added user {} to group {}",
            userid,
            managed_group
        ),
        Err(e) => {
            tracing::error!(
                "Could not add user {} to group {}. Error: {}",
                userid,
                managed_group,
                e
            );

            // this failed, so we need to remove the user so that we can try again
            // (there is a race condition here, but that would be fixed the next
            //  time the user is added)
            let kwargs = {
                let mut kwargs = HashMap::new();
                kwargs.insert("uid".to_string(), userid.clone());
                kwargs
            };

            match call_post::<IPAResponse>("user_del", None, Some(kwargs)).await {
                Ok(_) => {
                    tracing::info!(
                        "Successfully removed user {} after failed group add",
                        userid
                    )
                }
                Err(e) => {
                    tracing::error!(
                        "Could not remove user {} after failed group add. Error: {}",
                        userid,
                        e
                    );
                }
            }

            return Err(Error::Call(format!(
                "Could not add user {} to group {}. Error: {}",
                user, managed_group, e
            )));
        }
    }

    // now synchronise the groups - this won't do anything if another
    // thread has already beaten us to creating the user
    match sync_groups(&user).await {
        Ok(user) => {
            tracing::info!("Added user: {}", user);
            Ok(user)
        }
        Err(e) => {
            tracing::warn!(
                "Failed to add user {} - they have been removed from FreeIPA? {}",
                user.identifier(),
                e
            );
            Err(Error::Call(format!(
                "Failed to add user {} - they have been removed from FreeIPA? {}",
                user.identifier(),
                e
            )))
        }
    }
}

pub async fn update_homedir(user: &UserIdentifier, homedir: &str) -> Result<String, Error> {
    let homedir = homedir.trim();

    if homedir.is_empty() {
        return Err(Error::InvalidState("Empty homedir".to_string()));
    }

    // get the user from FreeIPA
    let user = get_user(user).await?.ok_or(Error::Call(format!(
        "User {} does not exist in FreeIPA?",
        user
    )))?;

    if user.home() == homedir {
        // nothing to do
        tracing::info!(
            "Homedir for user {} is already {}. No changes needed.",
            user.identifier(),
            homedir
        );
        return Ok(user.home().to_string());
    }

    // now update the homedir to the passed string
    let kwargs = {
        let mut kwargs = HashMap::new();
        kwargs.insert("uid".to_string(), user.userid().to_string());
        kwargs.insert("homedirectory".to_string(), homedir.to_string());
        kwargs
    };

    match call_post::<IPAResponse>("user_mod", None, Some(kwargs)).await {
        Ok(_) => {
            tracing::info!(
                "Successfully updated homedir for user: {}",
                user.identifier()
            );
        }
        Err(e) => {
            tracing::error!(
                "Could not update homedir for user {} to {}. Error: {}",
                user.identifier(),
                homedir,
                e
            );
        }
    }

    // now update the user in the cache
    let user = force_get_user(user.identifier())
        .await?
        .ok_or(Error::Call(format!(
            "User {} does not exist in FreeIPA?",
            user.identifier()
        )))?;

    if user.home() != homedir {
        return Err(Error::InvalidState(format!(
            "Homedir for user {} was not updated to {}",
            user, homedir
        )));
    }

    tracing::info!("User homedir updated: {}", user);

    Ok(user.home().to_string())
}
