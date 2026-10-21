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

///
/// An optional, platform-managed service installed alongside a project's own
/// tenancy - currently only inside a `VCluster`, so an add-on on a `Shared`
/// project is recorded but has no effect until it switches to `VCluster`.
///
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TenancyAddon {
    LlmIsvc,
    JupyterHub,
}

impl TenancyAddon {
    pub fn parse(s: &str) -> Result<Self, Error> {
        match s.trim().to_lowercase().as_str() {
            "llmisvc" => Ok(TenancyAddon::LlmIsvc),
            "jupyterhub" => Ok(TenancyAddon::JupyterHub),
            other => Err(Error::Parse(format!(
                "Invalid tenancy add-on '{}' - expected 'llmisvc' or 'jupyterhub'",
                other
            ))),
        }
    }
}

impl std::fmt::Display for TenancyAddon {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            TenancyAddon::LlmIsvc => write!(f, "llmisvc"),
            TenancyAddon::JupyterHub => write!(f, "jupyterhub"),
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

    #[test]
    fn test_tenancy_addon_round_trips_through_its_wire_form() {
        for addon in [TenancyAddon::LlmIsvc, TenancyAddon::JupyterHub] {
            assert_eq!(TenancyAddon::parse(&addon.to_string()).ok(), Some(addon));
        }

        assert_eq!(
            TenancyAddon::parse(" JupyterHub ").ok(),
            Some(TenancyAddon::JupyterHub)
        );
        assert!(TenancyAddon::parse("").is_err());
        assert!(TenancyAddon::parse("gateway").is_err());
    }
}
