// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use anyhow::Context;
use anyhow::Result;
use once_cell::sync::Lazy;
use reqwest::{cookie::Jar, Client};
use secrecy::{ExposeSecret, SecretString};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
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

            match error_name {
                "NotFound" => {
                    return Err(Error::NotFound(format!(
                        "Error in response: {:?}",
                        result.error
                    )));
                }
                "DuplicateEntry" => {
                    return Err(Error::Duplicate(format!(
                        "Error in response: {:?}",
                        result.error
                    )));
                }
                _ => {
                    return Err(Error::Call(format!(
                        "Error in response: {:?}",
                        result.error
                    )));
                }
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
    fn users(&self, project: &ProjectIdentifier) -> Result<Vec<IPAUser>, Error> {
        IPAUser::construct(&self.result.clone().unwrap_or_default(), project)
    }

    fn groups(&self) -> Result<Vec<IPAGroup>, Error> {
        IPAGroup::construct(&self.result.clone().unwrap_or_default())
    }

    fn internal_groups(
        &self,
        internal_groups: &HashMap<String, ProjectIdentifier>,
    ) -> Result<Vec<IPAGroup>, Error> {
        IPAGroup::construct_internal(&self.result.clone().unwrap_or_default(), internal_groups)
    }

    fn legacy_groups(&self, portal: &PortalIdentifier) -> Result<Vec<IPAGroup>, Error> {
        IPAGroup::construct_legacy(&self.result.clone().unwrap_or_default(), portal)
    }
}

#[derive(Debug, Clone)]
struct FreeAuth {
    server: String,
    jar: Arc<Jar>,
    user: String,
    password: SecretString,
    num_reconnects: u32,
}

impl FreeAuth {
    fn default() -> Self {
        FreeAuth {
            server: "".to_string(),
            jar: Arc::new(Jar::default()),
            user: "".to_string(),
            password: SecretString::default(),
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
async fn login(server: &str, user: &str, password: &SecretString) -> Result<Arc<Jar>, Error> {
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
    fn construct(
        result: &serde_json::Value,
        project: &ProjectIdentifier,
    ) -> Result<Vec<IPAUser>, Error> {
        let mut users = Vec::new();

        // convert result into an array if it isn't already
        let result = match result.as_array() {
            Some(result) => result.clone(),
            None => vec![result.clone()],
        };

        for user in result {
            let userid = user
                .get("uid")
                .and_then(|v| v.as_array())
                .and_then(|v| v.first())
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();

            if userid.is_empty() {
                tracing::error!("Could not find user id: Skipping user.",);
                continue;
            }

            let cn: &str = user
                .get("cn")
                .and_then(|v| v.as_array())
                .and_then(|v| v.first())
                .and_then(|v| v.as_str())
                .unwrap_or_default();

            let cn = match UserIdentifier::parse(cn) {
                Ok(cn) => match cn.project_identifier() == *project {
                    true => cn,
                    false => {
                        tracing::warn!("Skipping {} as they are not in project {}", cn, project);
                        continue;
                    }
                },
                Err(_) => {
                    // try to guess the user identifier from the username - support legacy
                    match UserIdentifier::parse(&format!(
                        "{}.{}",
                        userid,
                        project.portal_identifier()
                    )) {
                        Ok(cn) => match cn.project_identifier() == *project {
                            true => cn,
                            false => {
                                tracing::warn!(
                                    "Skipping {} as they are not in project {}",
                                    cn,
                                    project
                                );
                                continue;
                            }
                        },
                        Err(e) => {
                            tracing::error!(
                                "Could not parse user identifier: {}. Error: {}",
                                cn,
                                e
                            );
                            continue;
                        }
                    }
                }
            };

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
                    // are they a member of a legacy primary group?
                    let legacy_primary_group = format!("group.{}", project.project());

                    if memberof.contains(&legacy_primary_group) {
                        legacy_primary_group
                    } else {
                        tracing::debug!(
                            "Could not find primary group {} for user: {}",
                            primary_group,
                            cn
                        );
                        "".to_string()
                    }
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

        match std::env::var("OPENPORTAL_REQUIRE_MANAGED_CLASS") {
            Ok(value) => match value.to_lowercase().as_str() {
                "true" | "yes" | "1" => {
                    self.in_group(&managed_group) && self.userclass() == managed_group
                }
                _ => self.in_group(&managed_group),
            },
            Err(_) => self.in_group(&managed_group),
        }
    }

    ///
    /// Return whether or not a user is protected - they are
    /// protected if they are not managed
    ///
    pub fn is_protected(&self) -> bool {
        !self.is_managed()
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

    fn construct_internal(
        result: &serde_json::Value,
        internal_groups: &HashMap<String, ProjectIdentifier>,
    ) -> Result<Vec<IPAGroup>, Error> {
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

            let project = match internal_groups.get(&groupid) {
                Some(project) => project.clone(),
                None => {
                    let managed_group = get_managed_group()?;

                    match groupid == managed_group.groupid() {
                        true => managed_group.identifier().clone(),
                        false => continue,
                    }
                }
            };

            let description = group
                .get("description")
                .and_then(|v| v.as_array())
                .and_then(|v| v.first())
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();

            groups.push(IPAGroup {
                groupid,
                identifier: project,
                description,
            });
        }

        Ok(groups)
    }

    fn construct_legacy(
        result: &serde_json::Value,
        portal: &PortalIdentifier,
    ) -> Result<Vec<IPAGroup>, Error> {
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

            // this is a legacy group if the group name is "group.project"
            let parts: Vec<&str> = groupid.split('.').collect();

            if parts.len() != 2 {
                continue;
            }

            if parts[0] != "group" {
                continue;
            }

            let project = match ProjectIdentifier::parse(&format!("{}.{}", parts[1], portal)) {
                Ok(project) => project,
                Err(e) => {
                    tracing::warn!("Could not parse project: {}. Error: {}", parts[1], e);
                    continue;
                }
            };

            let mut description = group
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
                    Err(_) => {
                        description = format!("{} | {}", project, description);
                        project.clone()
                    }
                },
                None => {
                    description = format!("{} | {}", project, description);
                    project.clone()
                }
            };

            tracing::info!("Constructing legacy group {} / {}", groupid, identifier);

            groups.push(IPAGroup {
                groupid,
                identifier,
                description,
            });
        }

        Ok(groups)
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
                        tracing::debug!(
                            "Could not parse identifier: {} for {}. Error: {}",
                            identifier,
                            groupid,
                            e
                        );
                        continue;
                    }
                },
                None => {
                    tracing::debug!(
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

pub async fn connect(server: &str, user: &str, password: &SecretString) -> Result<(), Error> {
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
fn identifier_to_projectid(project: &ProjectIdentifier, legacy: bool) -> Result<String, Error> {
    // if the project.portal() is in ["openportal", "system", "instance"]
    // then we just return the project.project()
    let system_portals: Vec<String> = vec![
        "openportal".to_owned(),
        "system".to_owned(),
        "instance".to_owned(),
    ];

    if system_portals.contains(&project.portal()) {
        Ok(project.project().to_string())
    } else if legacy {
        // this is the legacy naming, `group.{project_name}`
        Ok(format!("group.{}", project.project()))
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

    // filter out users who are not enabled - we do list unmanaged users,
    // so that OpenPortal isn't repeatedly told to add users who already exist
    Ok(result
        .users(group.identifier())?
        .iter()
        .filter(|u| u.is_enabled())
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
                kwargs.insert("cn".to_string(), identifier_to_projectid(project, false)?);
                kwargs
            };

            let result = call_post::<IPAResponse>("group_find", None, Some(kwargs)).await?;

            if is_internal_portal(&project.portal()) {
                let internal_groups = cache::get_internal_group_ids().await?;

                match result.internal_groups(&internal_groups)?.first() {
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

                        // add this group to the cache
                        cache::add_existing_group(&group).await?;

                        Ok(Some(group))
                    }
                    None => Ok(None),
                }
            } else {
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
                    None => {
                        // try to find the legacy group - this is for porting tech/prep projects
                        let kwargs = {
                            let mut kwargs = HashMap::new();
                            kwargs
                                .insert("cn".to_string(), identifier_to_projectid(project, true)?);
                            kwargs
                        };

                        let result =
                            call_post::<IPAResponse>("group_find", None, Some(kwargs)).await?;

                        match result.legacy_groups(&project.portal_identifier())?.first() {
                            Some(group) => {
                                let group = match group.identifier() != project {
                                    true => {
                                        tracing::warn!(
                                        "Disagreement of identifier of found group: {} versus {}",
                                        group.identifier(),
                                        project
                                    );

                                        IPAGroup::new(
                                            group.groupid(),
                                            project,
                                            group.description(),
                                        )?
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
pub async fn identifier_to_userid(user: &UserIdentifier) -> Result<String, Error> {
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
        kwargs.insert("uid".to_string(), identifier_to_userid(user).await?);
        kwargs
    };

    let result = call_post::<IPAResponse>("user_find", None, Some(kwargs)).await?;

    match result.users(&user.project_identifier())?.first() {
        Some(user) => {
            cache::add_existing_user(user).await?;
            Ok(Some(user.clone()))
        }
        None => Ok(None),
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

    let internal_groups = cache::get_internal_group_ids().await?;

    let groups = result
        .groups()?
        .into_iter()
        .chain(result.internal_groups(&internal_groups)?.into_iter())
        .chain(
            result
                .legacy_groups(&user.identifier().portal_identifier())?
                .into_iter(),
        )
        .collect::<Vec<IPAGroup>>();

    // remove duplicates from this list
    let mut seen = HashSet::new();

    let groups = groups
        .into_iter()
        .filter(|g| seen.insert(g.groupid().to_string()))
        .collect::<Vec<IPAGroup>>();

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
pub fn get_managed_group() -> Result<IPAGroup, Error> {
    IPAGroup::new(
        "openportal",
        &ProjectIdentifier::parse("openportal.openportal")?,
        "Group for all users managed by OpenPortal",
    )
}

///
/// Return the group that indicates that OpenPortal is managing this user
/// for the resource controlled by the passed Peer
///
pub fn get_op_instance_group(peer: &Peer) -> Result<IPAGroup, Error> {
    let group_name = format!("op-{}", peer);

    // make sure that the group name only contains letters and numbers,
    // replacing @ with $ and all other characters with _
    let group_name = group_name
        .chars()
        .map(|c| match c {
            'a'..='z' | 'A'..='Z' | '0'..='9' => c,
            //'@' => '$',
            _ => '_',
        })
        .collect::<String>();

    let id = ProjectIdentifier::parse(&format!("{}.instance", group_name))?;

    IPAGroup::new(
        &identifier_to_projectid(&id, false)?,
        &id,
        "Group for users in OpenPortal who access this instance",
    )
}

///
/// Return the name of the primary group for the user
///
fn get_primary_group(user: &UserIdentifier) -> Result<IPAGroup, Error> {
    let project = user.project_identifier();

    IPAGroup::new(
        &identifier_to_projectid(&project, false)?,
        &project,
        &format!(
            "Primary group for all users in the {} project",
            project.project()
        ),
    )
}

pub async fn get_primary_group_name(user: &UserIdentifier) -> Result<String, Error> {
    let group = get_primary_group(user)?;

    Ok(group.groupid().to_string())
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

        group_cns.push(added_group.groupid().to_string());
    }

    // return the user in the system - check that the groups match
    let user = get_user(user.identifier())
        .await?
        .ok_or(Error::Call(format!(
            "User {} could not be found after adding?",
            user
        )))?;

    // We cannot do anything to a user who isn't enabled
    if user.is_disabled() {
        tracing::error!(
            "Cannot sync groups for user {} as they are disabled in FreeIPA.",
            user.userid()
        );

        return Err(Error::UnmanagedUser(format!(
            "Cannot sync groups for user {} as they are disabled in FreeIPA.",
            user.userid()
        )));
    }

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
/// Add the project to FreeIPA - this will create the group for the project
/// if it doesn't already exist. This returns the group
///
pub async fn add_project(project: &ProjectIdentifier) -> Result<IPAGroup, Error> {
    let project_group = get_group_create_if_not_exists(&IPAGroup::new(
        &identifier_to_projectid(project, false)?,
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
pub async fn remove_project(
    project: &ProjectIdentifier,
    instance: &Peer,
) -> Result<IPAGroup, Error> {
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
        if !user.is_managed() {
            tracing::warn!(
                "Ignoring user {} as they are not managed by OpenPortal",
                user.userid()
            );
            continue;
        }

        match remove_user(user.identifier(), instance).await {
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

    // DO NOT REMOVE THE GROUP AS WE MAY WANT TO RE-ADD IT LATER, AND
    // WILL NEED TO USE THE SAME GID!

    Ok(project_group)
}

async fn reenable_user(user: &IPAUser) -> Result<IPAUser, Error> {
    let kwargs = {
        let mut kwargs = HashMap::new();
        kwargs.insert("uid".to_string(), user.userid().to_string());
        kwargs
    };

    match call_post::<IPAResponse>("user_enable", None, Some(kwargs)).await {
        Ok(_) => {
            let mut user = user.clone();
            user.set_enabled();
            tracing::info!("Successfully re-enabled user: {}", user.identifier());
            // re-add the user to the cache
            cache::add_existing_user(&user).await?;
            Ok(user)
        }
        Err(Error::NotFound(_)) => {
            tracing::warn!(
                "User {} not found in FreeIPA. They have been removed behind our back and cannot be enabled.",
                user.identifier()
            );

            // invalidate the cache, as FreeIPA has been changed behind our back
            cache::clear().await?;

            Err(Error::NotFound(format!(
                "User {} not found in FreeIPA. They have been removed behind our back and \
                 cannot be enabled.",
                user.identifier()
            )))
        }
        Err(e) => {
            tracing::error!("Could not enable user: {}. Error: {}", user, e);

            // invalidate the cache, as FreeIPA has been changed behind our back
            cache::clear().await?;

            Err(Error::Call(format!(
                "Could not enable user: {}. Error: {}",
                user, e
            )))
        }
    }
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
pub async fn add_user(
    user: &UserIdentifier,
    instance: &Peer,
    homedir: &Option<String>,
) -> Result<IPAUser, Error> {
    // get a lock for this user, as only a single task should be adding
    // or removing this user at the same time
    let now = chrono::Utc::now();

    let _guard = loop {
        match cache::get_user_mutex(user).await?.try_lock_owned() {
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

                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }
        };
    };

    // return the up-to-date user if they already exist
    if let Some(mut user) = force_get_user(user).await? {
        if !user.is_managed() {
            tracing::warn!(
                "Ignoring request to add {} as they are not managed by OpenPortal",
                user.identifier()
            );

            // make sure to add the user to the cache
            cache::add_existing_user(&user).await?;

            return Ok(user);
        }

        // make sure to re-enable if needed
        if user.is_disabled() {
            user = match reenable_user(&user).await {
                Ok(user) => user,
                Err(e) => {
                    tracing::error!(
                        "Could not re-enable user {} after adding. Error: {}",
                        user.identifier(),
                        e
                    );

                    // return the original user that is not enabled
                    user
                }
            }
        }

        if user.is_managed() && user.is_enabled() {
            // make sure that the groups are correct for the existing user
            match sync_groups(&user, instance).await {
                Ok(user) => {
                    tracing::info!("Added user [cached] {}", user.identifier());
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

        // we get here if either the user isn't in FreeIPA, or there was
        // some problem re-enabling them. This means we will fall through
        // and will try to add the user from scratch
    }

    // Get the group that all managed users need to belong to
    let managed_group = get_managed_group()?;

    // The user doesn't exist, so try to add
    let mut kwargs = {
        let mut kwargs = HashMap::new();
        kwargs.insert("uid".to_string(), identifier_to_userid(user).await?);
        kwargs.insert("givenname".to_string(), user.username().to_string());
        kwargs.insert("sn".to_string(), user.project().to_string());
        kwargs.insert("userclass".to_string(), managed_group.groupid().to_string());
        kwargs.insert("cn".to_string(), user.to_string());

        kwargs
    };

    if let Some(homedir) = homedir {
        kwargs.insert("homedirectory".to_string(), homedir.to_string());
        tracing::info!("Adding user {} with home directory: {}", user, homedir);
    }

    let user = match call_post::<IPAResponse>("user_add", None, Some(kwargs)).await {
        Ok(result) => {
            tracing::info!("Successfully added user: {}", user);
            result.users(&user.project_identifier())?.first().cloned().ok_or(Error::UnmanagedUser(format!(
                "User {} could not be found after adding - this could be because they already exist, but aren't managed? \
                 Look for the user in FreeIPA and either add them to the managed group or removed them from FreeIPA.",
                user
            )))?
        }
        Err(Error::Duplicate(_)) => {
            // failed to add because the user already exists
            tracing::warn!(
                "Cannot add user {} as FreeIPA thinks they already exist",
                user
            );
            cache::clear().await?;

            match get_user(user).await? {
                Some(mut user) => {
                    if user.is_disabled() {
                        if user.is_managed() {
                            // the user should be enabled...
                            user = reenable_user(&user).await?;
                        } else {
                            tracing::warn!(
                                "User {} already exists in FreeIPA, but is not managed. \
                                Either add this user to the managed group, or remove them from FreeIPA.",
                                user
                            );

                            Err(Error::UnmanagedUser(
                                format!("User {} already exists in FreeIPA, but is not managed by OpenPortal. \
                                Either add this user to the managed group, or remove them from FreeIPA.", user)
                            ))?
                        }
                    }

                    user
                }
                None => {
                    tracing::warn!(
                        "Unable to fetch the user, despite them existing in FreeIPA. \
                            This is because the existing user is not managed. Either add \
                            this user to the managed group, or remove them from FreeIPA."
                    );

                    Err(Error::UnmanagedUser(
                        format!("User {} already exists in FreeIPA, but is not managed by OpenPortal. \
                                 Either add this user to the managed group, or remove them from FreeIPA.", user)
                    ))?
                }
            }
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
            // BUT we can only remove the user if they aren't in any other instance groups...
            for group in get_groups_for_user(&user).await? {
                if group.is_instance_group() {
                    tracing::warn!(
                        "User {} is in instance group {}. Cannot remove user after failed add.",
                        user.userid(),
                        group.groupid()
                    );
                    return Err(Error::UnmanagedUser(format!(
                        "User {} already exists in FreeIPA, but could not be added to the managed group. \
                        They are in the instance group {}. Either remove them from this group, or try again later.",
                        user.userid(),
                        group.groupid()
                    )));
                }
            }

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

    // now synchronise the groups
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
pub async fn remove_user(user: &UserIdentifier, instance: &Peer) -> Result<IPAUser, Error> {
    // get and lock a mutex on this user, as we should only have a single
    // task adding or removing this user at once
    let now = chrono::Utc::now();

    let _guard = loop {
        match cache::get_user_mutex(user).await?.try_lock_owned() {
            Ok(guard) => break guard,
            Err(_) => {
                if chrono::Utc::now().signed_duration_since(now).num_seconds() > 5 {
                    tracing::warn!(
                        "Could not get lock to remove user {} - another task is adding or removing.",
                        user
                    );

                    return Err(Error::Locked(format!(
                        "Could not get lock to remove user {} - another task is adding or removing.",
                        user
                    )));
                }

                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }
        };
    };

    // force get this user, as we need to have up-to-date information from FreeIPA
    let mut user = match force_get_user(user).await {
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

    if !user.is_managed() {
        tracing::warn!(
            "Ignoring request to remove {} as they are not managed by OpenPortal",
            user.identifier()
        );
        return Err(Error::UnmanagedUser(format!(
            "User {} is not managed by OpenPortal. Either add this user to the managed group, or remove them from FreeIPA.",
            user.identifier()
        )));
    }

    if user.is_disabled() {
        // nothing to do
        tracing::info!(
            "User {} is already disabled. No changes needed.",
            user.identifier()
        );
        return Ok(user);
    }

    // get the group used for openportal users of this peer
    let instance_group = get_op_instance_group(instance)?;

    // maybe don't do anything if the user isn't a member of this group
    if !user.in_group(instance_group.groupid()) {
        // check that they are in any groups...
        let in_other_instance_groups = match get_groups_for_user(&user).await {
            Ok(groups) => !groups
                .iter()
                .filter(|g| g.is_instance_group())
                .filter(|g| g.identifier() != instance_group.identifier())
                .collect::<Vec<&IPAGroup>>()
                .is_empty(),
            Err(e) => {
                tracing::error!("Could not get groups for user {}. Error: {}", user, e);
                false
            }
        };

        if in_other_instance_groups {
            tracing::warn!(
                "Ignoring request to remove {} as they are not in the instance group {}, but are in other resources",
                user.identifier(),
                instance_group.identifier(),
            );
            return Ok(user);
        }
    }

    // now remove the user from all of the instance groups for this peer
    let instance_groups = cache::get_instance_groups(instance).await?;

    for group in instance_groups {
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

    // refetch the groups for this user, as they will have changed
    let groups = match get_groups_for_user(&user).await {
        Ok(groups) => groups,
        Err(e) => {
            tracing::error!("Could not get groups for user {}. Error: {}", user, e);
            vec![]
        }
    };

    // get all of the other instance groups for this user
    let other_instance_groups = groups
        .iter()
        .filter(|g| g.is_instance_group())
        .filter(|g| g.identifier() != instance_group.identifier())
        .collect::<Vec<&IPAGroup>>();

    // don't remove the user if they are on different resources
    if !other_instance_groups.is_empty() {
        tracing::warn!(
            "Ignoring request to remove {} as they are in other resources: {:?}",
            user.identifier(),
            other_instance_groups
                .iter()
                .map(|g| g.identifier().to_string())
                .collect::<Vec<String>>()
        );

        // remove this user from the cache so that the list of users in this
        // project for this resource will be properly updated
        cache::remove_existing_user(&user).await?;

        return Ok(user);
    }

    // it is safe to remove the user - they aren't in any other resource

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
            tracing::info!("Successfully removed user: {}", user.identifier());
            cache::remove_existing_user(&user).await?;
        }
        Err(Error::NotFound(_)) => {
            tracing::info!(
                "User {} not found in FreeIPA. Assuming it has already been removed.",
                user.identifier()
            );

            // clear the cache as FreeIPA has been changed behind our back
            cache::clear().await?;
        }
        Err(e) => {
            tracing::error!("Could not remove user: {}. Error: {}", user.identifier(), e);
            return Err(Error::Call(format!(
                "Could not remove user: {}. Error: {}",
                user.identifier(),
                e
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

    if !user.is_managed() {
        tracing::warn!(
            "Ignoring request to update homedir for {} as they are not managed by OpenPortal",
            user.identifier()
        );
        return Ok(user.home().to_string());
    }

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

    // construct groups as the combination of both result.groups() and result.legacy_groups()
    let groups = result
        .groups()?
        .into_iter()
        .chain(result.legacy_groups(portal)?.into_iter())
        .collect::<Vec<IPAGroup>>();

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
pub async fn get_users(
    project: &ProjectIdentifier,
    instance: &Peer,
) -> Result<Vec<IPAUser>, Error> {
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

    let instance_group = get_op_instance_group(instance)?;
    let cached_users = cache::get_users_in_group(&project_group).await?;

    if !cached_users.is_empty() {
        // filter out users who are not in the instance group for this peer
        let users = cached_users
            .into_iter()
            .filter(|user| user.is_protected() || user.in_group(instance_group.groupid()))
            .collect::<Vec<IPAUser>>();

        return Ok(users);
    }

    // there are no users, meaning that we have not checked yet, or there
    // really are no users in this project...
    let users = force_get_users_in_group(&project_group).await?;

    cache::set_users_in_group(&project_group, &users).await?;

    // filter out users who are not in the instance group for this peer
    let users = users
        .into_iter()
        .filter(|user| user.is_protected() || user.in_group(instance_group.groupid()))
        .collect::<Vec<IPAUser>>();

    Ok(users)
}

pub async fn get_project_mapping(project: &ProjectIdentifier) -> Result<ProjectMapping, Error> {
    match get_group(project).await? {
        Some(group) => group.mapping(),
        None => Err(Error::MissingProject(format!(
            "Project {} does not exist in FreeIPA",
            project
        ))),
    }
}

pub async fn get_user_mapping(user: &UserIdentifier) -> Result<UserMapping, Error> {
    match get_user(user).await? {
        Some(user) => user.mapping(),
        None => Err(Error::MissingUser(format!(
            "User {} does not exist in FreeIPA",
            user
        ))),
    }
}

pub async fn is_protected_user(user: &UserIdentifier) -> Result<bool, Error> {
    // need to get the up-to-date version of the user,
    // in case their details have been changed in FreeIPA
    // behind our back. Important that we don't say a user
    // isn't protected when they have been manually removed from
    // the managed group...
    match force_get_user(user).await? {
        Some(user) => Ok(!user.is_managed()),
        None => Ok(false),
    }
}
