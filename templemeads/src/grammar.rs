// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use serde::{Deserialize, Serialize};

use crate::error::Error;

/// Grammar for all of the commands that can be sent to agents

///
/// A user identifier - this is a triple of username.project.portal
///
#[derive(Debug, Default, Clone, PartialEq, Eq, Hash)]
pub struct UserIdentifier {
    username: String,
    project: String,
    portal: String,
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

    pub fn is_valid(&self) -> bool {
        !self.username.is_empty() && !self.project.is_empty() && !self.portal.is_empty()
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
/// Struct that holds the mapping of a UserIdentifier to a local
/// username on a system
///
#[derive(Debug, Clone, PartialEq)]
pub struct UserMapping {
    user: UserIdentifier,
    local_user: String,
    local_project: String,
}

impl UserMapping {
    pub fn new(
        user: &UserIdentifier,
        local_user: &str,
        local_project: &str,
    ) -> Result<Self, Error> {
        let local_user = local_user.trim();
        let local_project = local_project.trim();

        if local_user.is_empty() {
            return Err(Error::Parse(format!(
                "Invalid UserMapping - local_user cannot be empty '{}'",
                local_user
            )));
        };

        if local_project.is_empty() {
            return Err(Error::Parse(format!(
                "Invalid UserMapping - local_project cannot be empty '{}'",
                local_project
            )));
        };

        Ok(Self {
            user: user.clone(),
            local_user: local_user.to_string(),
            local_project: local_project.to_string(),
        })
    }

    pub fn parse(identifier: &str) -> Result<Self, Error> {
        let parts: Vec<&str> = identifier.split(':').collect();

        if parts.len() != 3 {
            return Err(Error::Parse(format!("Invalid UserMapping: {}", identifier)));
        }

        let user = UserIdentifier::parse(parts[0])?;
        let local_user = parts[1].trim();
        let local_project = parts[2].trim();

        if local_user.is_empty() {
            return Err(Error::Parse(format!(
                "Invalid UserMapping - local_user cannot be empty '{}'",
                identifier
            )));
        };

        if local_project.is_empty() {
            return Err(Error::Parse(format!(
                "Invalid UserMapping - local_project cannot be empty '{}'",
                identifier
            )));
        };

        Ok(Self {
            user,
            local_user: local_user.to_string(),
            local_project: local_project.to_string(),
        })
    }

    pub fn user(&self) -> &UserIdentifier {
        &self.user
    }

    pub fn local_user(&self) -> &str {
        &self.local_user
    }

    pub fn local_project(&self) -> &str {
        &self.local_project
    }

    pub fn is_valid(&self) -> bool {
        self.user.is_valid() && !self.local_user.is_empty() && !self.local_project.is_empty()
    }
}

impl std::fmt::Display for UserMapping {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}:{}:{}",
            self.user, self.local_user, self.local_project
        )
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
    /// An instruction to add a user
    AddUser(UserIdentifier),

    /// An instruction to remove a user
    RemoveUser(UserIdentifier),

    /// An instruction to add a local user
    AddLocalUser(UserMapping),

    /// An instruction to remove a local user
    RemoveLocalUser(UserMapping),

    /// An instruction to update the home directory of a user
    UpdateHomeDir(UserIdentifier, String),

    /// Placeholder for an invalid instruction
    Invalid(String),
}

impl Instruction {
    pub fn new(s: &str) -> Self {
        let parts: Vec<&str> = s.split(' ').collect();
        match parts[0] {
            "add_user" => match UserIdentifier::parse(&parts[1..].join(" ")) {
                Ok(user) => Instruction::AddUser(user),
                Err(_) => Instruction::Invalid(s.to_string()),
            },
            "remove_user" => match UserIdentifier::parse(&parts[1..].join(" ")) {
                Ok(user) => Instruction::RemoveUser(user),
                Err(_) => Instruction::Invalid(s.to_string()),
            },
            "add_local_user" => match UserMapping::parse(&parts[1..].join(" ")) {
                Ok(mapping) => Instruction::AddLocalUser(mapping),
                Err(_) => Instruction::Invalid(s.to_string()),
            },
            "remove_local_user" => match UserMapping::parse(&parts[1..].join(" ")) {
                Ok(mapping) => Instruction::RemoveLocalUser(mapping),
                Err(_) => Instruction::Invalid(s.to_string()),
            },
            "update_homedir" => {
                if parts.len() < 3 {
                    return Instruction::Invalid(s.to_string());
                }

                let homedir = parts[2].trim().to_string();

                if homedir.is_empty() {
                    return Instruction::Invalid(s.to_string());
                }

                match UserIdentifier::parse(parts[1]) {
                    Ok(user) => Instruction::UpdateHomeDir(user, homedir),
                    Err(_) => Instruction::Invalid(s.to_string()),
                }
            }
            _ => Instruction::Invalid(s.to_string()),
        }
    }

    pub fn is_valid(&self) -> bool {
        match self {
            Instruction::AddUser(user) => user.is_valid(),
            Instruction::RemoveUser(user) => user.is_valid(),
            Instruction::AddLocalUser(mapping) => mapping.is_valid(),
            Instruction::RemoveLocalUser(mapping) => mapping.is_valid(),
            Instruction::UpdateHomeDir(user, homedir) => user.is_valid() && !homedir.is_empty(),
            Instruction::Invalid(_) => false,
        }
    }
}

impl std::fmt::Display for Instruction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Instruction::AddUser(user) => write!(f, "add_user {}", user),
            Instruction::RemoveUser(user) => write!(f, "remove_user {}", user),
            Instruction::AddLocalUser(mapping) => write!(f, "add_local_user {}", mapping),
            Instruction::RemoveLocalUser(mapping) => write!(f, "remove_local_user {}", mapping),
            Instruction::UpdateHomeDir(user, homedir) => {
                write!(f, "update_homedir {} {}", user, homedir)
            }
            Instruction::Invalid(s) => write!(f, "invalid {}", s),
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
        Ok(Instruction::new(&s))
    }
}
