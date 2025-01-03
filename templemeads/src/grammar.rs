// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use crate::destination::Destination;
use crate::error::Error;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// Grammar for all of the commands that can be sent to agents

pub trait NamedType {
    fn type_name() -> &'static str;
}

impl NamedType for String {
    fn type_name() -> &'static str {
        "String"
    }
}

impl NamedType for Vec<String> {
    fn type_name() -> &'static str {
        "Vec<String>"
    }
}

///
/// A portal identifier - this is just a string with no spaces or periods
///
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PortalIdentifier {
    portal: String,
}

impl NamedType for PortalIdentifier {
    fn type_name() -> &'static str {
        "PortalIdentifier"
    }
}

impl NamedType for Vec<PortalIdentifier> {
    fn type_name() -> &'static str {
        "Vec<PortalIdentifier>"
    }
}

impl PortalIdentifier {
    pub fn parse(identifier: &str) -> Result<Self, Error> {
        let portal = identifier.trim();

        if portal.is_empty() {
            return Err(Error::Parse(format!(
                "Invalid PortalIdentifier - portal cannot be empty '{}'",
                identifier
            )));
        };

        if portal.contains(' ') || portal.contains('.') {
            return Err(Error::Parse(format!(
                "Invalid PortalIdentifier - portal cannot contain spaces or periods '{}'",
                identifier
            )));
        };

        Ok(Self {
            portal: portal.to_string(),
        })
    }

    pub fn portal(&self) -> String {
        self.portal.clone()
    }
}

impl std::fmt::Display for PortalIdentifier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.portal)
    }
}

/// Serialize and Deserialize via the string representation
impl Serialize for PortalIdentifier {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        self.to_string().serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for PortalIdentifier {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        Self::parse(&s).map_err(serde::de::Error::custom)
    }
}

impl From<ProjectIdentifier> for PortalIdentifier {
    fn from(project: ProjectIdentifier) -> Self {
        project.portal_identifier()
    }
}

impl From<UserIdentifier> for PortalIdentifier {
    fn from(user: UserIdentifier) -> Self {
        user.portal_identifier()
    }
}

///
/// A project identifier - this is a double of project.portal
///
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ProjectIdentifier {
    project: String,
    portal: String,
}

impl NamedType for ProjectIdentifier {
    fn type_name() -> &'static str {
        "ProjectIdentifier"
    }
}

impl NamedType for Vec<ProjectIdentifier> {
    fn type_name() -> &'static str {
        "Vec<ProjectIdentifier>"
    }
}

impl ProjectIdentifier {
    pub fn parse(identifier: &str) -> Result<Self, Error> {
        let parts: Vec<&str> = identifier.split('.').collect();

        if parts.len() != 2 {
            return Err(Error::Parse(format!(
                "Invalid ProjectIdentifier: {}",
                identifier
            )));
        }

        let project = parts[0].trim();
        let portal = parts[1].trim();

        if project.is_empty() {
            return Err(Error::Parse(format!(
                "Invalid ProjectIdentifier - project cannot be empty '{}'",
                identifier
            )));
        };

        if portal.is_empty() {
            return Err(Error::Parse(format!(
                "Invalid ProjectIdentifier - portal cannot be empty '{}'",
                identifier
            )));
        };

        Ok(Self {
            project: project.to_string(),
            portal: portal.to_string(),
        })
    }

    pub fn project(&self) -> String {
        self.project.clone()
    }

    pub fn portal(&self) -> String {
        self.portal.clone()
    }

    pub fn portal_identifier(&self) -> PortalIdentifier {
        PortalIdentifier {
            portal: self.portal.clone(),
        }
    }
}

impl std::fmt::Display for ProjectIdentifier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}", self.project, self.portal)
    }
}

impl From<UserIdentifier> for ProjectIdentifier {
    fn from(user: UserIdentifier) -> Self {
        Self {
            project: user.project().to_string(),
            portal: user.portal().to_string(),
        }
    }
}

/// Serialize and Deserialize via the string representation

impl Serialize for ProjectIdentifier {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        self.to_string().serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for ProjectIdentifier {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        Self::parse(&s).map_err(serde::de::Error::custom)
    }
}

///
/// A user identifier - this is a triple of username.project.portal
///
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct UserIdentifier {
    username: String,
    project: String,
    portal: String,
}

impl NamedType for UserIdentifier {
    fn type_name() -> &'static str {
        "UserIdentifier"
    }
}

impl NamedType for Vec<UserIdentifier> {
    fn type_name() -> &'static str {
        "Vec<UserIdentifier>"
    }
}

impl UserIdentifier {
    pub fn parse(identifier: &str) -> Result<Self, Error> {
        let parts: Vec<&str> = identifier.split('.').collect();

        if parts.len() != 3 {
            return Err(Error::Parse(format!(
                "Invalid UserIdentifier: {}",
                identifier
            )));
        }

        let username = parts[0].trim();
        let project = parts[1].trim();
        let portal = parts[2].trim();

        if username.is_empty() {
            return Err(Error::Parse(format!(
                "Invalid UserIdentifier - username cannot be empty '{}'",
                identifier
            )));
        };

        if project.is_empty() {
            return Err(Error::Parse(format!(
                "Invalid UserIdentifier - project cannot be empty '{}'",
                identifier
            )));
        };

        if portal.is_empty() {
            return Err(Error::Parse(format!(
                "Invalid UserIdentifier - portal cannot be empty '{}'",
                identifier
            )));
        };

        Ok(Self {
            username: username.to_string(),
            project: project.to_string(),
            portal: portal.to_string(),
        })
    }

    pub fn username(&self) -> String {
        self.username.clone()
    }

    pub fn project(&self) -> String {
        self.project.clone()
    }

    pub fn portal(&self) -> String {
        self.portal.clone()
    }

    pub fn project_identifier(&self) -> ProjectIdentifier {
        ProjectIdentifier {
            project: self.project.clone(),
            portal: self.portal.clone(),
        }
    }

    pub fn portal_identifier(&self) -> PortalIdentifier {
        PortalIdentifier {
            portal: self.portal.clone(),
        }
    }
}

impl std::fmt::Display for UserIdentifier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}.{}", self.username, self.project, self.portal)
    }
}

/// Serialize and Deserialize via the string representation
/// of the UserIdentifier
impl Serialize for UserIdentifier {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        self.to_string().serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for UserIdentifier {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        Self::parse(&s).map_err(serde::de::Error::custom)
    }
}

///
/// Struct that holds the mapping of a ProjectIdentifier to a local
/// project on a system
///
#[derive(Debug, Clone, PartialEq)]
pub struct ProjectMapping {
    project: ProjectIdentifier,
    local_group: String,
}

impl NamedType for ProjectMapping {
    fn type_name() -> &'static str {
        "ProjectMapping"
    }
}

impl NamedType for Vec<ProjectMapping> {
    fn type_name() -> &'static str {
        "Vec<ProjectMapping>"
    }
}

impl ProjectMapping {
    pub fn new(project: &ProjectIdentifier, local_group: &str) -> Result<Self, Error> {
        let local_group = local_group.trim();

        if local_group.is_empty() {
            return Err(Error::Parse(format!(
                "Invalid ProjectMapping - local_group cannot be empty '{}'",
                local_group
            )));
        };

        if local_group.starts_with(".")
            || local_group.ends_with(".")
            || local_group.starts_with("/")
            || local_group.ends_with("/")
        {
            return Err(Error::Parse(format!(
                "Invalid ProjectMapping - local group contains invalid characters '{}'",
                local_group
            )));
        };

        Ok(Self {
            project: project.clone(),
            local_group: local_group.to_string(),
        })
    }

    pub fn parse(identifier: &str) -> Result<Self, Error> {
        let parts: Vec<&str> = identifier.split(':').collect();

        if parts.len() != 2 {
            return Err(Error::Parse(format!(
                "Invalid ProjectMapping: {}",
                identifier
            )));
        }

        let project = ProjectIdentifier::parse(parts[0])?;
        let local_group = parts[1].trim();

        Self::new(&project, local_group)
    }

    pub fn project(&self) -> &ProjectIdentifier {
        &self.project
    }

    pub fn local_group(&self) -> &str {
        &self.local_group
    }
}

impl std::fmt::Display for ProjectMapping {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{}", self.project, self.local_group)
    }
}

impl From<UserMapping> for ProjectMapping {
    fn from(mapping: UserMapping) -> Self {
        Self {
            project: mapping.user().project_identifier(),
            local_group: mapping.local_group().to_string(),
        }
    }
}

/// Serialize and Deserialize via the string representation

impl Serialize for ProjectMapping {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        self.to_string().serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for ProjectMapping {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        Self::parse(&s).map_err(serde::de::Error::custom)
    }
}

///
/// Struct that holds the mapping of a UserIdentifier to a local
/// username on a system
///
#[derive(Debug, Clone, PartialEq)]
pub struct UserMapping {
    user: UserIdentifier,
    local_user: String,
    local_group: String,
}

impl NamedType for UserMapping {
    fn type_name() -> &'static str {
        "UserMapping"
    }
}

impl NamedType for Vec<UserMapping> {
    fn type_name() -> &'static str {
        "Vec<UserMapping>"
    }
}

impl UserMapping {
    pub fn new(user: &UserIdentifier, local_user: &str, local_group: &str) -> Result<Self, Error> {
        let local_user = local_user.trim();
        let local_group = local_group.trim();

        if local_user.is_empty() {
            return Err(Error::Parse(format!(
                "Invalid UserMapping - local_user cannot be empty '{}'",
                local_user
            )));
        };

        if local_user.starts_with(".")
            || local_user.ends_with(".")
            || local_user.starts_with("/")
            || local_user.ends_with("/")
        {
            return Err(Error::Parse(format!(
                "Invalid UserMapping - local_user account contains invalid characters '{}'",
                local_user
            )));
        };

        if local_group.is_empty() {
            return Err(Error::Parse(format!(
                "Invalid UserMapping - local_group cannot be empty '{}'",
                local_group
            )));
        };

        if local_group.starts_with(".")
            || local_group.ends_with(".")
            || local_group.starts_with("/")
            || local_group.ends_with("/")
        {
            return Err(Error::Parse(format!(
                "Invalid UserMapping - local_group contains invalid characters '{}'",
                local_group
            )));
        };

        Ok(Self {
            user: user.clone(),
            local_user: local_user.to_string(),
            local_group: local_group.to_string(),
        })
    }

    pub fn parse(identifier: &str) -> Result<Self, Error> {
        let parts: Vec<&str> = identifier.split(':').collect();

        if parts.len() != 3 {
            return Err(Error::Parse(format!("Invalid UserMapping: {}", identifier)));
        }

        let user = UserIdentifier::parse(parts[0])?;
        let local_user = parts[1].trim();
        let local_group = parts[2].trim();

        Self::new(&user, local_user, local_group)
    }

    pub fn user(&self) -> &UserIdentifier {
        &self.user
    }

    pub fn local_user(&self) -> &str {
        &self.local_user
    }

    pub fn local_group(&self) -> &str {
        &self.local_group
    }
}

impl std::fmt::Display for UserMapping {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{}:{}", self.user, self.local_user, self.local_group)
    }
}

/// Serialize and Deserialize via the string representation
/// of the UserMapping
impl Serialize for UserMapping {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        self.to_string().serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for UserMapping {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        Self::parse(&s).map_err(serde::de::Error::custom)
    }
}

///
/// Enum of all of the instructions that can be sent to agents
///
#[derive(Debug, Clone, PartialEq)]
pub enum Instruction {
    /// An instruction to submit a job to the portal
    Submit(Destination, Arc<Instruction>),

    /// An instruction to get all projects managed by a portal
    GetProjects(PortalIdentifier),

    /// An instruction to add a project
    AddProject(ProjectIdentifier),

    /// An instruction to remove a project
    RemoveProject(ProjectIdentifier),

    /// An instruction to get all users in a project
    GetUsers(ProjectIdentifier),

    /// An instruction to add a user
    AddUser(UserIdentifier),

    /// An instruction to remove a user
    RemoveUser(UserIdentifier),

    /// An instruction to add a local user
    AddLocalUser(UserMapping),

    /// An instruction to remove a local user
    RemoveLocalUser(UserMapping),

    /// An instruction to add a local project
    AddLocalProject(ProjectMapping),

    /// An instruction to remove a local project
    RemoveLocalProject(ProjectMapping),

    /// An instruction to update the home directory of a user
    UpdateHomeDir(UserIdentifier, String),
}

impl Instruction {
    pub fn parse(s: &str) -> Result<Self, Error> {
        let parts: Vec<&str> = s.split(' ').collect();
        match parts[0] {
            "submit" => match Destination::parse(parts[1]) {
                Ok(destination) => match Instruction::parse(&parts[2..].join(" ")) {
                    Ok(instruction) => Ok(Instruction::Submit(
                        destination,
                        Arc::<Instruction>::new(instruction),
                    )),
                    Err(e) => {
                        tracing::error!(
                            "submit failed to parse the instruction for destination {}: {}. {}",
                            parts[1],
                            &parts[2..].join(" "),
                            e
                        );
                        Err(Error::Parse(format!(
                            "submit failed to parse the instruction for destination {}: {}. {}",
                            parts[1],
                            &parts[2..].join(" "),
                            e
                        )))
                    }
                },
                Err(e) => {
                    tracing::error!(
                        "submit failed to parse the destination for: {}. {}",
                        &parts[1..].join(" "),
                        e
                    );
                    Err(Error::Parse(format!(
                        "submit failed to parse the destination for: {}. {}",
                        &parts[1..].join(" "),
                        e
                    )))
                }
            },
            "get_projects" => match PortalIdentifier::parse(&parts[1..].join(" ")) {
                Ok(portal) => Ok(Instruction::GetProjects(portal)),
                Err(_) => {
                    tracing::error!("get_projects failed to parse: {}", &parts[1..].join(" "));
                    Err(Error::Parse(format!(
                        "get_projects failed to parse: {}",
                        &parts[1..].join(" ")
                    )))
                }
            },
            "add_project" => match ProjectIdentifier::parse(&parts[1..].join(" ")) {
                Ok(project) => Ok(Instruction::AddProject(project)),
                Err(_) => {
                    tracing::error!("add_project failed to parse: {}", &parts[1..].join(" "));
                    Err(Error::Parse(format!(
                        "add_project failed to parse: {}",
                        &parts[1..].join(" ")
                    )))
                }
            },
            "remove_project" => match ProjectIdentifier::parse(&parts[1..].join(" ")) {
                Ok(project) => Ok(Instruction::RemoveProject(project)),
                Err(_) => {
                    tracing::error!("remove_project failed to parse: {}", &parts[1..].join(" "));
                    Err(Error::Parse(format!(
                        "remove_project failed to parse: {}",
                        &parts[1..].join(" ")
                    )))
                }
            },
            "add_local_project" => match ProjectMapping::parse(&parts[1..].join(" ")) {
                Ok(mapping) => Ok(Instruction::AddLocalProject(mapping)),
                Err(_) => {
                    tracing::error!(
                        "add_local_project failed to parse: {}",
                        &parts[1..].join(" ")
                    );
                    Err(Error::Parse(format!(
                        "add_local_project failed to parse: {}",
                        &parts[1..].join(" ")
                    )))
                }
            },
            "remove_local_project" => match ProjectMapping::parse(&parts[1..].join(" ")) {
                Ok(mapping) => Ok(Instruction::RemoveLocalProject(mapping)),
                Err(_) => {
                    tracing::error!(
                        "remove_local_project failed to parse: {}",
                        &parts[1..].join(" ")
                    );
                    Err(Error::Parse(format!(
                        "remove_local_project failed to parse: {}",
                        &parts[1..].join(" ")
                    )))
                }
            },
            "get_users" => match ProjectIdentifier::parse(&parts[1..].join(" ")) {
                Ok(project) => Ok(Instruction::GetUsers(project)),
                Err(_) => {
                    tracing::error!("get_users failed to parse: {}", &parts[1..].join(" "));
                    Err(Error::Parse(format!(
                        "get_users failed to parse: {}",
                        &parts[1..].join(" ")
                    )))
                }
            },
            "add_user" => match UserIdentifier::parse(&parts[1..].join(" ")) {
                Ok(user) => Ok(Instruction::AddUser(user)),
                Err(_) => {
                    tracing::error!("add_user failed to parse: {}", &parts[1..].join(" "));
                    Err(Error::Parse(format!(
                        "add_user failed to parse: {}",
                        &parts[1..].join(" ")
                    )))
                }
            },
            "remove_user" => match UserIdentifier::parse(&parts[1..].join(" ")) {
                Ok(user) => Ok(Instruction::RemoveUser(user)),
                Err(_) => {
                    tracing::error!("remove_user failed to parse: {}", &parts[1..].join(" "));
                    Err(Error::Parse(format!(
                        "remove_user failed to parse: {}",
                        &parts[1..].join(" ")
                    )))
                }
            },
            "add_local_user" => match UserMapping::parse(&parts[1..].join(" ")) {
                Ok(mapping) => Ok(Instruction::AddLocalUser(mapping)),
                Err(_) => {
                    tracing::error!("add_local_user failed to parse: {}", &parts[1..].join(" "));
                    Err(Error::Parse(format!(
                        "add_local_user failed to parse: {}",
                        &parts[1..].join(" ")
                    )))
                }
            },
            "remove_local_user" => match UserMapping::parse(&parts[1..].join(" ")) {
                Ok(mapping) => Ok(Instruction::RemoveLocalUser(mapping)),
                Err(_) => {
                    tracing::error!(
                        "remove_local_user failed to parse: {}",
                        &parts[1..].join(" ")
                    );
                    Err(Error::Parse(format!(
                        "remove_local_user failed to parse: {}",
                        &parts[1..].join(" ")
                    )))
                }
            },
            "update_homedir" => {
                if parts.len() < 3 {
                    tracing::error!("update_homedir failed to parse: {}", &parts[1..].join(" "));
                    return Err(Error::Parse(format!(
                        "update_homedir failed to parse: {}",
                        &parts[1..].join(" ")
                    )));
                }

                let homedir = parts[2].trim().to_string();

                if homedir.is_empty() {
                    tracing::error!("update_homedir failed to parse: {}", &parts[1..].join(" "));
                    return Err(Error::Parse(format!(
                        "update_homedir failed to parse: {}",
                        &parts[1..].join(" ")
                    )));
                }

                match UserIdentifier::parse(parts[1]) {
                    Ok(user) => Ok(Instruction::UpdateHomeDir(user, homedir)),
                    Err(_) => {
                        tracing::error!(
                            "update_homedir failed to parse: {}",
                            &parts[1..].join(" ")
                        );
                        Err(Error::Parse(format!(
                            "update_homedir failed to parse: {}",
                            &parts[1..].join(" ")
                        )))
                    }
                }
            }
            _ => {
                tracing::error!("Invalid instruction: {}", s);
                Err(Error::Parse(format!("Invalid instruction: {}", s)))
            }
        }
    }
}

impl std::fmt::Display for Instruction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Instruction::Submit(destination, command) => {
                write!(f, "submit {} {}", destination, command)
            }
            Instruction::GetProjects(portal) => write!(f, "get_projects {}", portal),
            Instruction::AddProject(project) => write!(f, "add_project {}", project),
            Instruction::RemoveProject(project) => write!(f, "remove_project {}", project),
            Instruction::GetUsers(project) => write!(f, "get_users {}", project),
            Instruction::AddUser(user) => write!(f, "add_user {}", user),
            Instruction::RemoveUser(user) => write!(f, "remove_user {}", user),
            Instruction::AddLocalProject(mapping) => write!(f, "add_local_project {}", mapping),
            Instruction::RemoveLocalProject(mapping) => {
                write!(f, "remove_local_project {}", mapping)
            }
            Instruction::AddLocalUser(mapping) => write!(f, "add_local_user {}", mapping),
            Instruction::RemoveLocalUser(mapping) => write!(f, "remove_local_user {}", mapping),
            Instruction::UpdateHomeDir(user, homedir) => {
                write!(f, "update_homedir {} {}", user, homedir)
            }
        }
    }
}

/// Serialize and Deserialize via the string representation
/// of the Instructionimpl Serialize for Instruction {
impl Serialize for Instruction {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        self.to_string().serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for Instruction {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        match Instruction::parse(&s) {
            Ok(instruction) => Ok(instruction),
            Err(e) => Err(serde::de::Error::custom(e.to_string())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_user_identifier() {
        #[allow(clippy::unwrap_used)]
        let user = UserIdentifier::parse("user.project.portal").unwrap();
        assert_eq!(user.username(), "user");
        assert_eq!(user.project(), "project");
        assert_eq!(user.portal(), "portal");
        assert_eq!(user.to_string(), "user.project.portal");
    }

    #[test]
    fn test_user_mapping() {
        #[allow(clippy::unwrap_used)]
        let user = UserIdentifier::parse("user.project.portal").unwrap();
        #[allow(clippy::unwrap_used)]
        let mapping = UserMapping::new(&user, "local_user", "local_group").unwrap();
        assert_eq!(mapping.user(), &user);
        assert_eq!(mapping.local_user(), "local_user");
        assert_eq!(mapping.local_group(), "local_group");
        assert_eq!(
            mapping.to_string(),
            "user.project.portal:local_user:local_group"
        );
    }

    #[test]
    fn test_instruction() {
        #[allow(clippy::unwrap_used)]
        let user = UserIdentifier::parse("user.project.portal").unwrap();
        #[allow(clippy::unwrap_used)]
        let mapping = UserMapping::new(&user, "local_user", "local_group").unwrap();

        #[allow(clippy::unwrap_used)]
        let instruction = Instruction::parse("add_user user.project.portal").unwrap();
        assert_eq!(instruction, Instruction::AddUser(user.clone()));

        #[allow(clippy::unwrap_used)]
        let instruction = Instruction::parse("remove_user user.project.portal").unwrap();
        assert_eq!(instruction, Instruction::RemoveUser(user.clone()));

        #[allow(clippy::unwrap_used)]
        let instruction =
            Instruction::parse("add_local_user user.project.portal:local_user:local_group")
                .unwrap();
        assert_eq!(instruction, Instruction::AddLocalUser(mapping.clone()));

        #[allow(clippy::unwrap_used)]
        let instruction =
            Instruction::parse("remove_local_user user.project.portal:local_user:local_group")
                .unwrap();
        assert_eq!(instruction, Instruction::RemoveLocalUser(mapping.clone()));

        #[allow(clippy::unwrap_used)]
        let instruction =
            Instruction::parse("update_homedir user.project.portal /home/user").unwrap();
        assert_eq!(
            instruction,
            Instruction::UpdateHomeDir(user.clone(), "/home/user".to_string())
        );
    }

    #[test]
    fn assert_serialize_user() {
        #[allow(clippy::unwrap_used)]
        let user = UserIdentifier::parse("user.project.portal").unwrap();
        let serialized = serde_json::to_string(&user).unwrap_or_default();
        assert_eq!(serialized, "\"user.project.portal\"");
    }

    #[test]
    fn assert_deserialize_user() {
        #[allow(clippy::unwrap_used)]
        let user: UserIdentifier = serde_json::from_str("\"user.project.portal\"").unwrap();
        assert_eq!(user.to_string(), "user.project.portal");
    }

    #[test]
    fn assert_serialize_mapping() {
        #[allow(clippy::unwrap_used)]
        let user = UserIdentifier::parse("user.project.portal").unwrap();
        #[allow(clippy::unwrap_used)]
        let mapping = UserMapping::new(&user, "local_user", "local_group").unwrap();
        let serialized = serde_json::to_string(&mapping).unwrap_or_default();
        assert_eq!(serialized, "\"user.project.portal:local_user:local_group\"");
    }

    #[test]
    fn assert_deserialize_mapping() {
        #[allow(clippy::unwrap_used)]
        let mapping: UserMapping =
            serde_json::from_str("\"user.project.portal:local_user:local_group\"").unwrap();
        assert_eq!(
            mapping.to_string(),
            "user.project.portal:local_user:local_group"
        );
    }

    #[test]
    fn assert_serialize_instruction() {
        #[allow(clippy::unwrap_used)]
        let user = UserIdentifier::parse("user.project.portal").unwrap();
        #[allow(clippy::unwrap_used)]
        let mapping = UserMapping::new(&user, "local_user", "local_group").unwrap();

        let instruction = Instruction::AddUser(user.clone());
        let serialized = serde_json::to_string(&instruction).unwrap_or_default();
        assert_eq!(serialized, "\"add_user user.project.portal\"");

        let instruction = Instruction::RemoveUser(user.clone());
        let serialized = serde_json::to_string(&instruction).unwrap_or_default();
        assert_eq!(serialized, "\"remove_user user.project.portal\"");

        let instruction = Instruction::AddLocalUser(mapping.clone());
        let serialized = serde_json::to_string(&instruction).unwrap_or_default();
        assert_eq!(
            serialized,
            "\"add_local_user user.project.portal:local_user:local_group\""
        );

        let instruction = Instruction::RemoveLocalUser(mapping.clone());
        let serialized = serde_json::to_string(&instruction).unwrap_or_default();
        assert_eq!(
            serialized,
            "\"remove_local_user user.project.portal:local_user:local_group\""
        );

        let instruction = Instruction::UpdateHomeDir(user.clone(), "/home/user".to_string());
        let serialized = serde_json::to_string(&instruction).unwrap_or_default();
        assert_eq!(
            serialized,
            "\"update_homedir user.project.portal /home/user\""
        );
    }

    #[test]
    fn assert_deserialize_instruction() {
        #[allow(clippy::unwrap_used)]
        let user = UserIdentifier::parse("user.project.portal").unwrap();
        #[allow(clippy::unwrap_used)]
        let mapping = UserMapping::new(&user, "local_user", "local_group").unwrap();

        #[allow(clippy::unwrap_used)]
        let instruction: Instruction =
            serde_json::from_str("\"add_user user.project.portal\"").unwrap();
        assert_eq!(instruction, Instruction::AddUser(user.clone()));

        #[allow(clippy::unwrap_used)]
        let instruction: Instruction =
            serde_json::from_str("\"remove_user user.project.portal\"").unwrap();
        assert_eq!(instruction, Instruction::RemoveUser(user.clone()));

        #[allow(clippy::unwrap_used)]
        let instruction: Instruction =
            serde_json::from_str("\"add_local_user user.project.portal:local_user:local_group\"")
                .unwrap();
        assert_eq!(instruction, Instruction::AddLocalUser(mapping.clone()));

        #[allow(clippy::unwrap_used)]
        let instruction: Instruction = serde_json::from_str(
            "\"remove_local_user user.project.portal:local_user:local_group\"",
        )
        .unwrap();
        assert_eq!(instruction, Instruction::RemoveLocalUser(mapping.clone()));

        #[allow(clippy::unwrap_used)]
        let instruction: Instruction =
            serde_json::from_str("\"update_homedir user.project.portal /home/user\"").unwrap();
        assert_eq!(
            instruction,
            Instruction::UpdateHomeDir(user.clone(), "/home/user".to_string())
        );
    }
}
