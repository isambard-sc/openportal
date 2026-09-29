// SPDX-FileCopyrightText: © 2026 David John <David.John@bristol.ac.uk>
// SPDX-License-Identifier: MIT

//!
//! Connects to a single Kubernetes cluster's API server with a bearer token
//! and CRUDs `Project`/`ProjectUser` custom resources. Everything else -
//! namespaces, RBAC, quotas - is composed on-cluster by KRO from these two
//! objects, so that is all this agent ever needs to touch.
//!

use anyhow::Context;
use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use greatwestern::grammar::{ProjectMapping, UserMapping};
use kube::api::{Api, DeleteParams, Patch, PatchParams};
use kube::config::{AuthInfo, Kubeconfig};
use kube::{Client, Config};
use once_cell::sync::OnceCell;
use secrecy::SecretString;
use templemeads::Error;

use crate::resources::{project_user_name, Project, ProjectSpec, ProjectUser, ProjectUserSpec};

static CLIENT: OnceCell<Client> = OnceCell::new();

/// The field manager name used for every server-side apply this agent makes -
/// lets the API server tell this agent's writes apart from KRO's or an
/// operator's `kubectl apply`.
const FIELD_MANAGER: &str = "op-k8s";

///
/// Connect to the Kubernetes cluster's API server, and remember the
/// resulting client for the lifetime of this process.
///
/// Tries the pod's own in-cluster ServiceAccount first (the standard
/// `KUBERNETES_SERVICE_HOST`/`_PORT` env vars plus the token/CA kubelet
/// mounts at `/var/run/secrets/kubernetes.io/serviceaccount/` - present when
/// this agent runs as a workload on the cluster it manages). That token is
/// re-read from disk per request rather than cached, so it picks up
/// kubelet's rotation automatically - unlike the static, external token
/// below, it never goes stale.
///
/// Falls back to the explicit `api_server`/`ca_bundle`/`token` (all three
/// required together) when not running in-cluster - e.g. this agent running
/// outside the cluster it manages, authenticating with a manually-extracted
/// service account token.
///
pub async fn connect(
    api_server: Option<&str>,
    ca_bundle: Option<&str>,
    token: Option<SecretString>,
) -> Result<(), Error> {
    // This agent's kube client needs a process-wide rustls crypto provider
    // installed before paddington's own event loop starts (this call happens
    // first in `main`). `Err` just means one is already installed - see the
    // matching comment in `paddington::eventloop::run`.
    let _ = rustls::crypto::ring::default_provider().install_default();

    let config = match kube::Config::incluster() {
        Ok(config) => {
            tracing::info!("Running in-cluster - using this pod's own ServiceAccount credentials");
            config
        }
        Err(e) => {
            tracing::debug!(
                "Not running in-cluster ({}) - falling back to explicit configuration",
                e
            );

            let (api_server, ca_bundle, token) = match (api_server, ca_bundle, token) {
                (Some(api_server), Some(ca_bundle), Some(token)) => (api_server, ca_bundle, token),
                _ => {
                    return Err(Error::InvalidConfig(
                        "Not running in-cluster, and k8s-api-server/k8s-ca-bundle/k8s-token \
                         are not all set. Either run this agent inside the cluster it \
                         manages, or set all three of those options."
                            .to_string(),
                    ));
                }
            };

            build_external_config(api_server, ca_bundle, token).await?
        }
    };

    let client = Client::try_from(config)
        .with_context(|| "Could not build a Kubernetes client from the config")?;

    CLIENT
        .set(client)
        .map_err(|_| Error::Bug("Kubernetes client has already been connected".to_string()))?;

    Ok(())
}

async fn build_external_config(
    api_server: &str,
    ca_bundle: &str,
    token: SecretString,
) -> Result<Config, Error> {
    let kubeconfig = Kubeconfig::from_yaml(&format!(
        r#"
apiVersion: v1
kind: Config
clusters:
  - name: cluster
    cluster:
      server: {api_server}
      certificate-authority-data: {ca_bundle_b64}
contexts:
  - name: context
    context:
      cluster: cluster
      user: user
current-context: context
users:
  - name: user
    user: {{}}
"#,
        api_server = api_server,
        ca_bundle_b64 = BASE64.encode(ca_bundle.as_bytes()),
    ))
    .with_context(|| "Could not build a kubeconfig from the configured API server and CA bundle")?;

    let mut config = Config::from_custom_kubeconfig(kubeconfig, &Default::default())
        .await
        .with_context(|| "Could not build a client config from the kubeconfig")?;

    config.auth_info = AuthInfo {
        token: Some(token),
        ..Default::default()
    };

    Ok(config)
}

fn client() -> Result<Client, Error> {
    CLIENT
        .get()
        .cloned()
        .ok_or_else(|| Error::Bug("Kubernetes client has not been connected".to_string()))
}

fn projects_api() -> Result<Api<Project>, Error> {
    Ok(Api::all(client()?))
}

fn project_users_api() -> Result<Api<ProjectUser>, Error> {
    Ok(Api::all(client()?))
}

/// Idempotently create (or update, if it already exists) the `Project`
/// custom resource for this mapping. Every field it doesn't set is left for
/// the CRD's own defaults, applied by the API server.
pub async fn upsert_project(mapping: &ProjectMapping) -> Result<(), Error> {
    let project = Project::new(mapping.local_group(), ProjectSpec::default());

    projects_api()?
        .patch(
            mapping.local_group(),
            &PatchParams::apply(FIELD_MANAGER).force(),
            &Patch::Apply(&project),
        )
        .await
        .with_context(|| format!("Could not create/update Project {}", mapping.local_group()))?;

    Ok(())
}

/// Deletes the `Project`, if it exists. A missing `Project` counts as
/// already removed, not an error - `RemoveLocalProject` must be safe to
/// retry.
pub async fn delete_project(mapping: &ProjectMapping) -> Result<(), Error> {
    match projects_api()?
        .delete(mapping.local_group(), &DeleteParams::default())
        .await
    {
        Ok(_) => Ok(()),
        Err(kube::Error::Api(e)) if e.code == 404 => Ok(()),
        Err(e) => Err(Error::Any(anyhow::Error::new(e).context(format!(
            "Could not delete Project {}",
            mapping.local_group()
        )))),
    }
}

pub async fn project_exists(mapping: &ProjectMapping) -> Result<bool, Error> {
    match projects_api()?.get(mapping.local_group()).await {
        Ok(_) => Ok(true),
        Err(kube::Error::Api(e)) if e.code == 404 => Ok(false),
        Err(e) => Err(Error::Any(
            anyhow::Error::new(e)
                .context(format!("Could not get Project {}", mapping.local_group())),
        )),
    }
}

pub async fn get_project_node_hours(mapping: &ProjectMapping) -> Result<u64, Error> {
    let project = projects_api()?
        .get(mapping.local_group())
        .await
        .with_context(|| format!("Could not get Project {}", mapping.local_group()))?;

    Ok(project
        .spec
        .budget
        .map(|budget| budget.node_hours)
        .unwrap_or(0))
}

pub async fn set_project_node_hours(
    mapping: &ProjectMapping,
    node_hours: u64,
) -> Result<(), Error> {
    let patch = serde_json::json!({
        "spec": {
            "budget": {
                "nodeHours": node_hours,
            }
        }
    });

    projects_api()?
        .patch(
            mapping.local_group(),
            &PatchParams::apply(FIELD_MANAGER),
            &Patch::Merge(&patch),
        )
        .await
        .with_context(|| {
            format!(
                "Could not set the node-hour budget for Project {}",
                mapping.local_group()
            )
        })?;

    Ok(())
}

/// Idempotently create (or update) the `ProjectUser` custom resource for
/// this mapping, named `<project>-<username>` by convention.
pub async fn upsert_project_user(mapping: &UserMapping) -> Result<(), Error> {
    let username = mapping.local_user().unix()?;
    let name = project_user_name(mapping.local_group(), username);

    let project_user = ProjectUser::new(
        &name,
        ProjectUserSpec {
            project: mapping.local_group().to_string(),
            username: username.to_string(),
            uid: None,
            gid: None,
            blocked: None,
        },
    );

    project_users_api()?
        .patch(
            &name,
            &PatchParams::apply(FIELD_MANAGER).force(),
            &Patch::Apply(&project_user),
        )
        .await
        .with_context(|| format!("Could not create/update ProjectUser {}", name))?;

    Ok(())
}

/// Deletes the `ProjectUser`, if it exists. A missing `ProjectUser` counts
/// as already removed, not an error - `RemoveLocalUser` must be safe to
/// retry.
pub async fn delete_project_user(mapping: &UserMapping) -> Result<(), Error> {
    let username = mapping.local_user().unix()?;
    let name = project_user_name(mapping.local_group(), username);

    match project_users_api()?
        .delete(&name, &DeleteParams::default())
        .await
    {
        Ok(_) => Ok(()),
        Err(kube::Error::Api(e)) if e.code == 404 => Ok(()),
        Err(e) => Err(Error::Any(
            anyhow::Error::new(e).context(format!("Could not delete ProjectUser {}", name)),
        )),
    }
}

pub async fn project_user_exists(mapping: &UserMapping) -> Result<bool, Error> {
    let username = mapping.local_user().unix()?;
    let name = project_user_name(mapping.local_group(), username);

    match project_users_api()?.get(&name).await {
        Ok(_) => Ok(true),
        Err(kube::Error::Api(e)) if e.code == 404 => Ok(false),
        Err(e) => Err(Error::Any(
            anyhow::Error::new(e).context(format!("Could not get ProjectUser {}", name)),
        )),
    }
}
