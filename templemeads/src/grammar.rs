// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use serde::{Deserialize, Serialize};

/// Grammar for all of the commands that can be sent to agents

///
/// A user identifier - this is a triple of username.project.portal
///
#[derive(Debug, Clone, PartialEq)]
pub struct UserIdentifier {
    username: String,
    project: String,
    portal: String,
}

impl UserIdentifier {
    pub fn new(identifier: &str) -> Self {
        let parts: Vec<&str> = identifier.split('.').collect();

        if parts.len() != 3 {
            tracing::error!("Invalid UserIdentifier: {}", identifier);
            return Self {
                username: "".to_string(),
                project: "".to_string(),
                portal: "".to_string(),
            };
        }

        Self {
            username: parts[0].to_string(),
            project: parts[1].to_string(),
            portal: parts[2].to_string(),
        }
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
        Ok(Self::new(&s))
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

    /// Placeholder for an invalid instruction
    Invalid(String),
}

impl Instruction {
    pub fn new(s: &str) -> Self {
        let parts: Vec<&str> = s.split(' ').collect();
        match parts[0] {
            "add_user" => {
                let user = UserIdentifier::new(&parts[1..].join(" "));
                Instruction::AddUser(user)
            }
            "remove_user" => {
                let user = UserIdentifier::new(&parts[1..].join(" "));
                Instruction::RemoveUser(user)
            }
            _ => Instruction::Invalid(s.to_string()),
        }
    }

    pub fn is_valid(&self) -> bool {
        match self {
            Instruction::AddUser(user) => !user.username().is_empty(),
            Instruction::RemoveUser(user) => !user.username().is_empty(),
            Instruction::Invalid(_) => false,
        }
    }
}

impl std::fmt::Display for Instruction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Instruction::AddUser(user) => write!(f, "add_user {}", user),
            Instruction::RemoveUser(user) => write!(f, "remove_user {}", user),
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
