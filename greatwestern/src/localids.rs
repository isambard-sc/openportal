// SPDX-FileCopyrightText: © 2026 David John <David.John@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use serde::{Deserialize, Serialize};
use templemeads::named::NamedType;
use ts_rs::TS;

///
/// The numeric ids a local user has on a system, as resolved by the agent
/// that owns that system's view of the name service (the filesystem agent).
///
/// `user_gid` is the user's own private group - the group with the same name
/// as the user, which with FreeIPA user-private-groups has a GID equal to
/// the UID. `project_gid` is the GID of the mapping's `local_group`, i.e. the
/// project group the user is a member of (e.g. `project.brics`).
///
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct LocalUserIds {
    uid: u32,
    user_gid: u32,
    project_gid: u32,
}

impl NamedType for LocalUserIds {
    fn type_name() -> String {
        "LocalUserIds".to_string()
    }
}

impl LocalUserIds {
    pub fn new(uid: u32, user_gid: u32, project_gid: u32) -> Self {
        Self {
            uid,
            user_gid,
            project_gid,
        }
    }

    pub fn uid(&self) -> u32 {
        self.uid
    }

    pub fn user_gid(&self) -> u32 {
        self.user_gid
    }

    pub fn project_gid(&self) -> u32 {
        self.project_gid
    }
}

impl std::fmt::Display for LocalUserIds {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "uid={} user_gid={} project_gid={}",
            self.uid, self.user_gid, self.project_gid
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_local_user_ids_round_trip() {
        let ids = LocalUserIds::new(1483800125, 1483800125, 1483800002);
        let json = serde_json::to_string(&ids).unwrap_or_else(|e| unreachable!("{e}"));
        let back: LocalUserIds =
            serde_json::from_str(&json).unwrap_or_else(|e| unreachable!("{e}"));
        assert_eq!(ids, back);
        assert_eq!(back.uid(), 1483800125);
        assert_eq!(back.user_gid(), 1483800125);
        assert_eq!(back.project_gid(), 1483800002);
    }
}
