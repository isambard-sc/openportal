// SPDX-FileCopyrightText: © 2026 David John <David.John@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use templemeads::Error;

///
/// How a project's users are given access to a platform that supports more
/// than one form of tenancy - currently only Kubernetes, where `Shared` is a
/// namespace on the host cluster and `VCluster` is a virtual cluster of the
/// project's own.
///
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tenancy {
    Shared,
    VCluster,
}

impl Tenancy {
    pub fn parse(s: &str) -> Result<Self, Error> {
        match s.trim().to_lowercase().as_str() {
            "shared" => Ok(Tenancy::Shared),
            "vcluster" => Ok(Tenancy::VCluster),
            other => Err(Error::Parse(format!(
                "Invalid tenancy '{}' - expected 'shared' or 'vcluster'",
                other
            ))),
        }
    }
}

impl std::fmt::Display for Tenancy {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            Tenancy::Shared => write!(f, "shared"),
            Tenancy::VCluster => write!(f, "vcluster"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tenancy_round_trips_through_its_wire_form() {
        for tenancy in [Tenancy::Shared, Tenancy::VCluster] {
            assert_eq!(Tenancy::parse(&tenancy.to_string()).ok(), Some(tenancy));
        }

        assert_eq!(Tenancy::parse(" VCluster ").ok(), Some(Tenancy::VCluster));
        assert!(Tenancy::parse("").is_err());
        assert!(Tenancy::parse("dedicated").is_err());
    }
}
