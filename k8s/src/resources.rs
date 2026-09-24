// SPDX-FileCopyrightText: © 2026 David John <David.John@bristol.ac.uk>
// SPDX-License-Identifier: MIT

//!
//! Rust types for the `Project` and `ProjectUser` custom resources that KRO
//! composes into namespaces, RBAC and quotas on the Kubernetes cluster. This
//! agent only ever CRUDs these two objects - KRO's `ResourceGraphDefinition`
//! (running on-cluster, outside OpenPortal) does everything else.
//!
//! Optional spec fields are left unset on create wherever the CRD's schema
//! (as drafted by the platform team) already carries a default, so the API
//! server fills them in rather than this agent duplicating them.
//!

use k8s_openapi::apimachinery::pkg::apis::meta::v1::Condition;
use kube::CustomResource;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(CustomResource, Serialize, Deserialize, Clone, Debug, Default, JsonSchema)]
#[kube(
    group = "platform.isambard.ac.uk",
    version = "v1alpha1",
    kind = "Project",
    plural = "projects",
    status = "ProjectStatus"
)]
#[serde(rename_all = "camelCase")]
pub struct ProjectSpec {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub r#type: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub quota: Option<ProjectQuota>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub budget: Option<ProjectBudget>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub end_date: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub sso_group_suffix: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub sync_user_services: Option<ProjectSyncUserServices>,
}

#[derive(Serialize, Deserialize, Clone, Debug, JsonSchema)]
pub struct ProjectQuota {
    pub storage: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProjectBudget {
    pub node_hours: u64,
}

#[derive(Serialize, Deserialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProjectSyncUserServices {
    pub workflow_templates: bool,
    pub resource_claim_templates: bool,
}

/// Generic KRO-composed status: conditions plus whatever KRO reports as the
/// composition's readiness. Not written by this agent, only read.
#[derive(Serialize, Deserialize, Clone, Debug, Default, JsonSchema)]
pub struct ProjectStatus {
    #[serde(default)]
    pub conditions: Vec<Condition>,
}

#[derive(CustomResource, Serialize, Deserialize, Clone, Debug, JsonSchema)]
#[kube(
    group = "platform.isambard.ac.uk",
    version = "v1alpha1",
    kind = "ProjectUser",
    plural = "projectusers",
    status = "ProjectUserStatus"
)]
#[serde(rename_all = "camelCase")]
pub struct ProjectUserSpec {
    /// The owning `Project`'s name (== its namespace). `ProjectUser` is
    /// cluster-scoped, so this can't be inferred from the object's own
    /// namespace the way a Project's children can.
    pub project: String,
    pub username: String,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub uid: Option<u32>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub gid: Option<u32>,

    /// Suspends the user's project-namespace RBAC without deleting their
    /// identity or usage history. `RemoveLocalUser` deletes the object
    /// outright rather than setting this - see `client::delete_project_user`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub blocked: Option<bool>,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, JsonSchema)]
pub struct ProjectUserStatus {
    #[serde(default)]
    pub conditions: Vec<Condition>,
}

/// `ProjectUser` objects are cluster-scoped and named `<project>-<username>`
/// by convention - see `k8s/src/client.rs`.
pub fn project_user_name(project: &str, username: &str) -> String {
    format!("{}-{}", project, username)
}
