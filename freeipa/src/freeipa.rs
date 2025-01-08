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
use templemeads::grammar::{
    PortalIdentifier, ProjectIdentifier, ProjectMapping, UserIdentifier, UserMapping,
};
use templemeads::Error;
use tokio::sync::{Mutex, MutexGuard};

use templemeads::agent::Peer;

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
        .danger_accept_invalid_certs(should_allow_invalid_certs())
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
                    .danger_accept_invalid_certs(should_allow_invalid_certs())
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
            let error_name: &str = result
                .error
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or_default();

            if error_name == "NotFound" {
                return Err(Error::NotFound(format!(
                    "Error in response: {:?}",
                    result.error
                )));
            } else {
                return Err(Error::Call(format!(
                    "Error in response: {:?}",
                    result.error
                )));
            }
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

fn should_allow_invalid_certs() -> bool {
    match std::env::var("OPENPORTAL_ALLOW_INVALID_SSL_CERTS") {
        Ok(value) => value.to_lowercase() == "true",
        Err(_) => false,
    }
}

///
/// Login to the FreeIPA server using the passed username and password.
/// This returns a cookie jar that will contain the resulting authorisation
/// cookie, and which can be used for subsequent calls to the server.
///
async fn login(server: &str, user: &str, password: &Secret<String>) -> Result<Arc<Jar>, Error> {
    let jar = Arc::new(Jar::default());

    let client = Client::builder()
        .cookie_provider(Arc::clone(&jar))
        .danger_accept_invalid_certs(should_allow_invalid_certs())
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

#[derive(Debug, Clone)]
pub struct IPAUser {
    userid: String,
    cn: UserIdentifier,
    givenname: String,
    homedirectory: String,
    userclass: String,
    primary_group: String,
    memberof: Vec<String>,
    enabled: bool,
}

// implement display for IPAUser
impl std::fmt::Display for IPAUser {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}: local_name={}, givenname={}, userclass={}, primary_group={}, memberof={}, home={}, enabled={}",
            self.identifier(),
            self.userid(),
            self.givenname(),
            self.userclass(),
            self.primary_group(),
            self.memberof().join(","),
            self.home(),
            self.is_enabled()
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
            let cn: &str = match user
                .get("cn")
                .and_then(|v| v.as_array())
                .and_then(|v| v.first())
                .and_then(|v| v.as_str())
            {
                Some(cn) => cn,
                None => {
                    tracing::error!("Could not parse user identifier (CN) from: {}", user);
                    continue;
                }
            };

            let cn = match UserIdentifier::parse(cn) {
                Ok(cn) => cn,
                Err(_) => {
                    tracing::error!(
                        "Could not parse user identifier from CN: {cn} :Skipping user."
                    );
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
            let primary_group = get_primary_group(&cn)?.groupid().to_string();

            let primary_group = match memberof.contains(&primary_group) {
                true => primary_group,
                false => {
                    tracing::warn!(
                        "Could not find primary group {} for user: {}",
                        primary_group,
                        cn
                    );
                    "".to_string()
                }
            };

            // try to see if this user is enabled - the nsaccountlock
            // attribute is changed to "True" when the account is disabled
            let disabled = user
                .get("nsaccountlock")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);

            users.push(IPAUser {
                userid,
                cn,
                givenname,
                userclass,
                homedirectory,
                primary_group,
                memberof,
                enabled: !disabled,
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
        if self.primary_group.is_empty() {
            // this is a user that doesn't have a primary group - likely because
            // they were disabled. We can guess the primary group, which
            // we will do here, printing out a warning if the user isn't disabled
            let guessed_primary_group = get_primary_group(&self.cn)?.groupid().to_string();

            if self.is_enabled() {
                tracing::warn!(
                    "User {} does not have a primary group. Guessing: {}",
                    self.identifier(),
                    guessed_primary_group
                );
            }

            UserMapping::new(&self.cn, self.userid(), &guessed_primary_group)
        } else {
            UserMapping::new(&self.cn, self.userid(), self.primary_group())
        }
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
            Ok(group) => group.groupid().to_string(),
            Err(_) => return false,
        };

        self.in_group(&managed_group) && self.userclass() == managed_group
    }

    ///
    /// Return whether or not this user is enabled in FreeIPA
    ///
    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    ///
    /// Return whether or not this user is disabled in FreeIPA
    ///
    pub fn is_disabled(&self) -> bool {
        !self.is_enabled()
    }

    ///
    /// Set this user as enabled in FreeIPA
    ///
    pub fn set_enabled(&mut self) {
        self.enabled = true;
    }

    ///
    /// Set this user as disabled in FreeIPA
    ///
    pub fn set_disabled(&mut self) {
        self.enabled = false;
    }
}

#[derive(Debug, Clone)]
pub struct IPAGroup {
    groupid: String,
    identifier: ProjectIdentifier,
    description: String,
}

// implement display for IPAGroup
impl std::fmt::Display for IPAGroup {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}: identifier={} description={}",
            self.groupid(),
            self.identifier(),
            self.description()
        )
    }
}

impl IPAGroup {
    fn new(
        groupid: &str,
        identifier: &ProjectIdentifier,
        description: &str,
    ) -> Result<Self, Error> {
        // check that the groupid is valid .... PARSING RULES
        let groupid = groupid.trim();
        let description = description.trim();

        if groupid.is_empty() {
            return Err(Error::Parse("Group identifier is empty".to_string()));
        }

        if description.is_empty() {
            return Err(Error::Parse("Group description is empty".to_string()));
        }

        Ok(IPAGroup {
            groupid: groupid.to_string(),
            identifier: identifier.clone(),
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

            // get the identifier from the description (if possible)
            let identifier = match description.split("|").next() {
                Some(identifier) => match ProjectIdentifier::parse(identifier.trim()) {
                    Ok(identifier) => identifier,
                    Err(e) => {
                        tracing::warn!("Could not parse identifier: {}. Error: {}", identifier, e);
                        continue;
                    }
                },
                None => {
                    tracing::warn!(
                        "Could not parse identifier from description: {}",
                        description
                    );
                    continue;
                }
            };

            groups.push(IPAGroup {
                groupid,
                identifier,
                description,
            });
        }

        Ok(groups)
    }

    pub fn parse_system_groups(groups: &str) -> Result<Vec<IPAGroup>, Error> {
        let groups = groups.trim();

        if groups.is_empty() {
            return Ok(Vec::new());
        }

        let mut g = Vec::new();
        let mut errors = Vec::new();

        for group in groups.split(',') {
            if !group.is_empty() {
                let project_id = ProjectIdentifier::parse(&format!("{}.system", group))?;

                match IPAGroup::new(group, &project_id, "OpenPortal-managed group") {
                    Ok(group) => g.push(group),
                    Err(e) => {
                        tracing::error!("Could not parse group: {}. Error: {}", group, e);
                        errors.push(group)
                    }
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

    pub fn parse_instance_groups(groups: &str) -> Result<HashMap<Peer, Vec<IPAGroup>>, Error> {
        let groups = groups.trim();

        if groups.is_empty() {
            return Ok(HashMap::new());
        }

        let mut g: HashMap<Peer, Vec<IPAGroup>> = HashMap::new();
        let mut errors = Vec::new();

        for group in groups.split(',') {
            let parts: Vec<&str> = group.split(':').collect();

            if parts.len() != 2 {
                errors.push(group);
                continue;
            }

            let instance = parts[0].trim();

            if instance.is_empty() {
                errors.push(group);
                continue;
            }

            let peer = match Peer::parse(instance) {
                Ok(peer) => peer,
                Err(e) => {
                    tracing::error!("Could not parse instance: {}. Error: {}", instance, e);
                    errors.push(group);
                    continue;
                }
            };

            let group = parts[1].trim();

            if group.is_empty() {
                errors.push(group);
                continue;
            }

            let project_id = ProjectIdentifier::parse(&format!("{}.instance", group))?;

            match IPAGroup::new(group, &project_id, "OpenPortal-managed group") {
                Ok(group) => {
                    if let Some(groups) = g.get_mut(&peer) {
                        groups.push(group);
                    } else {
                        g.insert(peer.clone(), vec![group]);
                    }
                }
                Err(e) => {
                    tracing::error!("Could not parse group: {}. Error: {}", group, e);
                    errors.push(group)
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

    pub fn identifier(&self) -> &ProjectIdentifier {
        &self.identifier
    }

    pub fn groupid(&self) -> &str {
        &self.groupid
    }

    pub fn mapping(&self) -> Result<ProjectMapping, Error> {
        ProjectMapping::new(&self.identifier, &self.groupid)
    }

    pub fn description(&self) -> &str {
        &self.description
    }

    pub fn is_system_group(&self) -> bool {
        self.identifier.portal() == "system"
    }

    pub fn is_instance_group(&self) -> bool {
        self.identifier.portal() == "instance"
    }

    pub fn is_project_group(&self) -> bool {
        !(self.is_system_group() || self.is_instance_group())
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
/// Return whether this is an internal reserved portal name, i.e.
/// "openportal", "system", or "instance"
///
fn is_internal_portal(portal: &str) -> bool {
    matches!(portal, "openportal" | "system" | "instance")
}

///
/// Return the Unix project name associated with the passed ProjectIdentifier.
///
/// Eventually we will need to deal with federation, and work
/// out a way to uniquely convert project identifiers to Unix groups.
/// Currently, for the identifier group.portal, we require that
/// a portal ensures group is unique within the portal. For now,
/// we will just use group (as brics is the only portal)
///
/// Note that we also have system groups, which are of the form
/// system.group, and instance groups, which are of the form
/// instance.group. These two names should be reserved and not
/// used for any portals
///
fn identifier_to_projectid(project: &ProjectIdentifier) -> Result<String, Error> {
    // if the project.portal() is in ["openportal", "system", "instance"]
    // then we just return the project.project()
    let system_portals: Vec<String> = vec![
        "openportal".to_owned(),
        "system".to_owned(),
        "instance".to_owned(),
    ];

    if system_portals.contains(&project.portal()) {
        Ok(project.project().to_string())
    } else {
        Ok(format!("{}.{}", project.portal(), project.project()))
    }
}

///
/// Return all of the users who are part of the specified group
///
async fn force_get_users_in_group(group: &IPAGroup) -> Result<Vec<IPAUser>, Error> {
    if !group.is_project_group() {
        // we only list users in project groups
        return Ok(Vec::new());
    }

    let kwargs = {
        let mut kwargs = HashMap::new();
        kwargs.insert("in_group".to_string(), group.groupid().to_string());
        kwargs.insert("all".to_string(), "true".to_string());
        kwargs.insert("sizelimit".to_string(), "2048".to_string());
        kwargs
    };

    let result = call_post::<IPAResponse>("user_find", None, Some(kwargs)).await?;

    // filter out users who are not managed and who are disabled
    Ok(result
        .users()?
        .iter()
        .filter(|u| u.is_managed() & u.is_enabled())
        .cloned()
        .collect())
}

///
/// Return the specified group from FreeIPA, or None if it does
/// not exist
///
async fn get_group(project: &ProjectIdentifier) -> Result<Option<IPAGroup>, Error> {
    match cache::get_group(project).await? {
        Some(group) => Ok(Some(group)),
        None => {
            let kwargs = {
                let mut kwargs = HashMap::new();
                kwargs.insert("cn".to_string(), identifier_to_projectid(project)?);
                kwargs
            };

            let result = call_post::<IPAResponse>("group_find", None, Some(kwargs)).await?;

            match result.groups()?.first() {
                Some(group) => {
                    let group = match group.identifier() != project {
                        true => {
                            tracing::warn!(
                                "Disagreement of identifier of found group: {} versus {}",
                                group.identifier(),
                                project
                            );

                            IPAGroup::new(group.groupid(), project, group.description())?
                        }
                        false => group.clone(),
                    };

                    // add this group to the cache - also force get all
                    // of the users currently in this group
                    cache::add_existing_group(&group).await?;

                    // if this is a project group, then get and cache all users
                    // in this group
                    if group.is_project_group() {
                        let users = force_get_users_in_group(&group).await?;
                        cache::set_users_in_group(&group, &users).await?;
                    }

                    Ok(Some(group))
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
    // only get users whose portals are not in the internal set
    if is_internal_portal(&user.portal()) {
        return Ok(None);
    }

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
    let all_users = result.users()?;

    let users: Vec<IPAUser> = all_users
        .clone()
        .into_iter()
        .filter(|user| user.is_managed())
        .collect();

    match users.first() {
        Some(user) => {
            cache::add_existing_user(user).await?;
            Ok(Some(user.clone()))
        }
        None => {
            if !all_users.is_empty() {
                tracing::warn!(
                    "User {} not found in FreeIPA, but found {} user(s) that matched who were not managed.",
                    user,
                    all_users.len()
                );

                for user in all_users {
                    tracing::warn!("User: {}, is_managed={}", user, user.is_managed());
                }
            }

            Ok(None)
        }
    }
}

///
/// Return all of the groups that the user is a member of
///
async fn get_groups_for_user(user: &IPAUser) -> Result<Vec<IPAGroup>, Error> {
    let kwargs = {
        let mut kwargs = HashMap::new();
        kwargs.insert("user".to_string(), user.userid().to_string());
        kwargs.insert("all".to_string(), "true".to_string());
        kwargs.insert("sizelimit".to_string(), "2048".to_string());
        kwargs
    };

    let result = call_post::<IPAResponse>("group_find", None, Some(kwargs)).await?;

    let groups = result.groups()?;

    tracing::info!(
        "User {} is in groups: {:?}",
        user.identifier(),
        groups.iter().map(|g| g.groupid()).collect::<Vec<&str>>()
    );

    Ok(groups)
}

///
/// Return the user matching the passed identifier - note that
/// this will only return users who are managed (part of the
/// "openportal" group)
///
async fn get_user(user: &UserIdentifier) -> Result<Option<IPAUser>, Error> {
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

    // it doesn't - try to create the group - we will encode the ProjectIdentifier
    // in the description
    let description = format!("{} | {}", group.identifier(), group.description());

    let kwargs = {
        let mut kwargs = HashMap::new();
        kwargs.insert("cn".to_string(), group.groupid().to_string());
        kwargs.insert("description".to_string(), description);
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
    IPAGroup::new(
        "openportal",
        &ProjectIdentifier::parse("openportal.openportal")?,
        "Group for all users managed by OpenPortal",
    )
}

///
/// Return the name of the primary group for the user
///
fn get_primary_group(user: &UserIdentifier) -> Result<IPAGroup, Error> {
    let project = user.project_identifier();

    IPAGroup::new(
        &identifier_to_projectid(&project)?,
        &project,
        &format!(
            "Primary group for all users in the {} project",
            project.project()
        ),
    )
}

///
/// Call this function to synchronise the groups for the passed user.
/// This checks that the user is in the correct groups, and adds them
/// or removes them as necessary. Groups will match the project group,
/// the system groups, and the openportal group.
///
async fn sync_groups(user: &IPAUser, instance: &Peer) -> Result<IPAUser, Error> {
    // the user probably doesn't exist, so add them, making sure they
    // are in the correct groups
    let mut groups = cache::get_system_groups().await?;

    // add in the groups for this instance
    groups.extend(cache::get_instance_groups(instance).await?);

    // add in the "openportal" group, to which all users managed by
    // OpenPortal must belong
    groups.push(get_managed_group()?);

    // also add in the group for the user's project (this is their primary group)
    groups.push(get_primary_group(user.identifier())?);

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

        group_cns.push(group.groupid().to_string());
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

    // and also cache that this user is a member of the project groups
    let project_groups: Vec<IPAGroup> = groups
        .into_iter()
        .filter(|g| g.is_project_group())
        .collect();

    cache::add_user_to_groups(&user, &project_groups).await?;

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

///
/// Functions in the freeipa public API
///

///
/// Add the project to FreeIPA - this will create the group for the project
/// if it doesn't already exist. This returns the group
///
pub async fn add_project(project: &ProjectIdentifier) -> Result<IPAGroup, Error> {
    let project_group = get_group_create_if_not_exists(&IPAGroup::new(
        &identifier_to_projectid(project)?,
        project,
        "OpenPortal-managed group",
    )?)
    .await?;

    Ok(project_group)
}

///
/// Remove the project from FreeIPA - this will remove the group for the project
/// if it exists, returning the removed group if successful,
/// or it will return an error if it doesn't exist, or something else
/// goes wrong
///
pub async fn remove_project(project: &ProjectIdentifier) -> Result<IPAGroup, Error> {
    let project_group = match get_group(project).await {
        Ok(Some(group)) => group,
        Ok(None) => {
            tracing::warn!(
                "Could not find group for project {}. Assuming it has already been removed.",
                project
            );
            return Err(Error::NotFound(format!(
                "Could not find group for project {}. Assuming it has already been removed.",
                project
            )));
        }
        Err(e) => {
            tracing::error!("Could not find group for project {}. Error: {}", project, e);
            return Err(Error::Call(format!(
                "Could not find group for project {}. Error: {}",
                project, e
            )));
        }
    };

    if !project_group.is_project_group() {
        return Err(Error::InvalidState(format!(
            "Cannot remove the group {} associated with project {} because it is not a project group?",
            project_group, project)));
    }

    // now get all of the users in this project and remove them as well!
    let users = force_get_users_in_group(&project_group).await?;

    tracing::info!(
        "Removing group {} for project {}",
        project_group.groupid(),
        project
    );

    for user in users {
        match remove_user(user.identifier()).await {
            Ok(user) => {
                tracing::info!("Successfully removed group user: {}", user);
            }
            Err(e) => {
                tracing::error!(
                    "Could not remove user {} who is a member of project group {}. Error: {}",
                    user.userid(),
                    project_group.groupid(),
                    e
                );
            }
        };
    }

    let kwargs = {
        let mut kwargs = HashMap::new();
        kwargs.insert("cn".to_string(), project_group.groupid().to_string());
        kwargs
    };

    match call_post::<IPAResponse>("group_del", None, Some(kwargs)).await {
        Ok(_) => {
            tracing::info!("Successfully removed group: {}", project_group);
            cache::remove_existing_group(&project_group).await?;
        }
        // match a Error::NotFound here
        Err(Error::NotFound(_)) => {
            tracing::info!(
                "Group {} not found in FreeIPA. Assuming it has already been removed.",
                project_group
            );

            // invalidate the cache, as FreeIPA has been changed behind our back
            cache::clear().await?;
        }
        Err(e) => {
            tracing::error!("Could not remove group: {}. Error: {}", project_group, e);
            return Err(Error::Call(format!(
                "Could not remove group: {}. Error: {}",
                project_group, e
            )));
        }
    }

    Ok(project_group)
}

///
/// Add the passed user to FreeIPA, added from the passed peer instance.
/// This will return the added user if successful, or will return an
/// error if something goes wrong. This returns the existing user if
/// they are already in FreeIPA. Note that this will only work for
/// users that are managed by OpenPortal, i.e. there will be an error
/// if there is an exising user with the same name, but which is not
/// managed by OpenPortal
///
pub async fn add_user(user: &UserIdentifier, instance: &Peer) -> Result<IPAUser, Error> {
    // return the user if they already exist
    if let Some(mut user) = get_user(user).await? {
        // make sure that they are enabled if they are disabled
        if user.is_disabled() {
            let kwargs = {
                let mut kwargs = HashMap::new();
                kwargs.insert("uid".to_string(), user.userid().to_string());
                kwargs
            };

            match call_post::<IPAResponse>("user_enable", None, Some(kwargs)).await {
                Ok(_) => {
                    user.set_enabled();
                    tracing::info!("Successfully re-enabled user: {}", user);
                    // re-add the user to the cache
                    cache::add_existing_user(&user).await?;

                    // make sure that the groups are correct
                    match sync_groups(&user, instance).await {
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
                }
                Err(Error::NotFound(_)) => {
                    tracing::info!(
                        "User {} not found in FreeIPA. They have been removed behind our back and cannot be enabled.",
                        user
                    );

                    tracing::info!("We will try to add this user again...");

                    // invalidate the cache, as FreeIPA has been changed behind our back
                    cache::clear().await?;
                }
                Err(e) => {
                    tracing::error!("Could not enable user: {}. Error: {}", user, e);
                    tracing::info!(
                        "We will try to add user {} again, as FreeIPA is clearly broken for this user.",
                        user.identifier()
                    );

                    // invalidate the cache, as FreeIPA has been changed behind our back
                    cache::clear().await?;
                }
            }
        } else {
            // make sure that the groups are correct for the existing user
            match sync_groups(&user, instance).await {
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
        kwargs.insert("userclass".to_string(), managed_group.groupid().to_string());
        kwargs.insert("cn".to_string(), user.to_string());

        kwargs
    };

    // try to add the user...
    let user = match call_post::<IPAResponse>("user_add", None, Some(kwargs)).await {
        Ok(result) => {
            tracing::info!("Successfully added user: {}", user);
            result.users()?.first().cloned().ok_or(Error::Call(format!(
                "User {} could not be found after adding - this could be because they already exist, but aren't managed?",
                user
            )))?
        }
        Err(e) => {
            // failed to add - maybe they already exist?
            tracing::error!("Could not add user: {}. Error: {}", user, e);
            match get_user(user).await? {
                Some(user) => {
                    tracing::info!("User already exists: {}", user);
                    user
                }
                None => {
                    return Err(Error::Call(format!(
                        "Could not add user: {}. Error: {}",
                        user, e
                    )));
                }
            }
        }
    };

    // add this user to the managed group so that it can be managed
    let userid = user.userid().to_string();

    match loop {
        // make sure that this group exists
        let managed_group = get_group_create_if_not_exists(&managed_group).await?;

        let kwargs = {
            let mut kwargs = HashMap::new();
            kwargs.insert("cn".to_string(), managed_group.groupid().to_string());
            kwargs.insert("user".to_string(), userid.clone());
            kwargs
        };

        match call_post::<IPAResponse>("group_add_member", None, Some(kwargs)).await {
            Ok(_) => {
                break Ok(());
            }
            Err(Error::NotFound(_)) => {
                tracing::warn!(
                    "Group {} not found in FreeIPA. Assuming it has been removed - clearing cache and re-adding.",
                    managed_group
                );
                cache::clear().await?;
            }
            Err(e) => {
                break Err(e);
            }
        }
    } {
        Ok(_) => {
            tracing::info!(
                "Successfully added user {} to group {}",
                userid,
                managed_group
            );
        }
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

            match call_post::<IPAResponse>("user_disable", None, Some(kwargs)).await {
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
    let mut attempts = 0;

    match loop {
        attempts += 1;

        if attempts > 3 {
            break Err(Error::Call(format!(
                "Failed to synchronise groups for user {} after 3 attempts",
                user.identifier()
            )));
        }

        match sync_groups(&user, instance).await {
            Ok(user) => {
                tracing::info!("Added user: {}", user);
                break Ok(user);
            }
            Err(Error::NotFound(e)) => {
                tracing::warn!(
                "User {} or groups not found in FreeIPA. They have been removed? Clearing cache and re-adding. Error: {}",
                user, e
            );
                cache::clear().await?;
            }
            Err(e) => {
                tracing::warn!(
                    "Failed to synchronise groups for user {}: {}",
                    user.identifier(),
                    e
                );
                break Err(Error::Call(format!(
                    "Failed to synchronise groups for user {}: {}",
                    user.identifier(),
                    e
                )));
            }
        }
    } {
        Ok(user) => Ok(user),
        Err(e) => Err(e),
    }
}

///
/// Remove the user from FreeIPA - this will return the removed user if
/// successful, or will return an error if the user doesn't exist, or
/// something else goes wrong. Note that the user must be managed by
/// OpenPortal, or an error will be returned
///
pub async fn remove_user(user: &UserIdentifier) -> Result<IPAUser, Error> {
    let mut user = match get_user(user).await {
        Ok(Some(user)) => user,
        Ok(None) => {
            tracing::warn!(
                "Could not find user {}. Assuming they have already been removed.",
                user
            );
            return Err(Error::NotFound(format!(
                "Could not find user {}. Assuming they have already been removed.",
                user
            )));
        }
        Err(e) => {
            tracing::error!("Could not find user {}. Error: {}", user, e);
            return Err(Error::Call(format!(
                "Could not find user {}. Error: {}",
                user, e
            )));
        }
    };

    if user.is_disabled() {
        // nothing to do
        tracing::info!("User {} is already disabled. No changes needed.", user);
        return Ok(user);
    }

    // get all of the groups that this user is in
    let groups = match get_groups_for_user(&user).await {
        Ok(groups) => groups,
        Err(e) => {
            tracing::error!("Could not get groups for user {}. Error: {}", user, e);
            vec![]
        }
    };

    // remove the user from all groups EXCEPT the managed group
    // This is necessary to make sure that we don't accidentally
    // add the user back to groups they don't have permission to be
    // in if they are re-enabled
    let managed_group = get_managed_group()?;

    for group in groups {
        if group.identifier() == managed_group.identifier() {
            continue;
        }

        let kwargs = {
            let mut kwargs = HashMap::new();
            kwargs.insert("cn".to_string(), group.groupid().to_string());
            kwargs.insert("user".to_string(), user.userid().to_string());
            kwargs
        };

        match call_post::<IPAResponse>("group_remove_member", None, Some(kwargs)).await {
            Ok(_) => {
                tracing::info!(
                    "Successfully removed user {} from group {}",
                    user.identifier(),
                    group.groupid()
                );
            }
            Err(Error::NotFound(_)) => {
                tracing::info!(
                    "Group {} not found in FreeIPA. Assuming it has already been removed.",
                    group
                );

                cache::clear().await?;
            }
            Err(e) => {
                tracing::error!(
                    "Could not remove user {} from group {}. Error: {}",
                    user.identifier(),
                    group.groupid(),
                    e
                );
            }
        }
    }

    let kwargs = {
        let mut kwargs = HashMap::new();
        kwargs.insert("uid".to_string(), user.userid().to_string());
        kwargs
    };

    // we don't actually remove users - instead we disable them so that
    // they can't log in. This way, if the user is re-added, then they
    // will get the same UID and other details
    match call_post::<IPAResponse>("user_disable", None, Some(kwargs)).await {
        Ok(_) => {
            user.set_disabled();
            tracing::info!("Successfully removed user: {}", user);
            cache::remove_existing_user(&user).await?;
        }
        Err(Error::NotFound(_)) => {
            tracing::info!(
                "User {} not found in FreeIPA. Assuming it has already been removed.",
                user
            );

            // clear the cache as FreeIPA has been changed behind our back
            cache::clear().await?;
        }
        Err(e) => {
            tracing::error!("Could not remove user: {}. Error: {}", user, e);
            return Err(Error::Call(format!(
                "Could not remove user: {}. Error: {}",
                user, e
            )));
        }
    }

    Ok(user)
}

///
/// Update the homedir for the user - this will return the updated homedir
/// if successful, or will return an error if the user doesn't exist, or
/// something else goes wrong. Note that the user must be managed by
/// OpenPortal, or an error will be returned
///
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
        Err(Error::NotFound(_)) => {
            tracing::info!(
                "User {} not found in FreeIPA. Assuming it has been removed behind our back.",
                user
            );

            // clear the cache as FreeIPA has been changed behind our back
            cache::clear().await?;
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

///
/// Return all of the groups that are managed by OpenPortal for the
/// passed portal
///
pub async fn get_groups(portal: &PortalIdentifier) -> Result<Vec<IPAGroup>, Error> {
    tracing::info!("Getting managed groups for portal: {}", portal);
    if is_internal_portal(&portal.portal()) {
        // return an empty set of groups for internal portals
        return Ok(Vec::new());
    }

    // calling group_find with no arguments should list all groups
    // I don't like setting a high size limit, but I am unsure how to
    // get all groups otherwise, as freeipa doesn't look like it has
    // a paging option? This could in theory be reduced by searching
    // for groups using a glob pattern, e.g. "portal.*"
    let kwargs = {
        let mut kwargs = HashMap::new();
        kwargs.insert("sizelimit".to_string(), "2048".to_string());
        kwargs
    };

    let result = call_post::<IPAResponse>("group_find", None, Some(kwargs)).await?;

    let groups = result.groups()?;

    cache::add_existing_groups(&groups).await?;

    Ok(groups
        .iter()
        .filter(|group| group.identifier().portal() == portal.portal())
        .cloned()
        .collect())
}

///
/// Return all of the users that are managed by OpenPortal for the
/// passed project. Note that this will only return users who are
/// managed by OpenPortal
///
pub async fn get_users(project: &ProjectIdentifier) -> Result<Vec<IPAUser>, Error> {
    tracing::info!("Getting users for project: {}", project);

    // don't get the users for project identifiers that use internal portal names
    // as they aren't public
    if is_internal_portal(&project.portal()) {
        return Ok(Vec::new());
    }

    let project_group = match get_group(project).await {
        Ok(Some(group)) => group,
        Ok(None) => {
            tracing::warn!(
                "Could not find group for project {}. Assuming it has already been removed.",
                project
            );
            return Ok(vec![]);
        }
        Err(e) => {
            tracing::error!("Could not find group for project {}. Error: {}", project, e);
            return Err(Error::Call(format!(
                "Could not find group for project {}. Error: {}",
                project, e
            )));
        }
    };

    let cached_users = cache::get_users_in_group(&project_group).await?;

    if !cached_users.is_empty() {
        return Ok(cached_users);
    }

    // there are no users, meaning that we have not checked yet, or there
    // really are no users in this project...
    let users = force_get_users_in_group(&project_group).await?;

    cache::set_users_in_group(&project_group, &users).await?;

    Ok(users)
}
