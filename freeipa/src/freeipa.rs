// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use anyhow::Result;
use anyhow::{Context, Error as AnyError};
use reqwest::{cookie::Jar, Client};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::sync::Arc;
use thiserror::Error;

async fn call_get<T>(url: &str) -> Result<T, Error>
where
    T: DeserializeOwned,
{
    let result = Client::new()
        .get(url)
        .send()
        .await
        .with_context(|| format!("Could not call function: {}", url))?;

    tracing::info!("Response: {:?}", result);

    if result.status().is_success() {
        Ok(result
            .json::<T>()
            .await
            .context("Could not decode from json")?)
    } else {
        Err(Error::Call(format!(
            "Could not get response for function: {}. Status: {}. Response: {:?}",
            url,
            result.status(),
            result
        )))
    }
}

/*

curl -v  \
        -H referer:https://$IPAHOSTNAME/ipa  \
        -H "Content-Type:application/x-www-form-urlencoded" \
        -H "Accept:text/plain"\
        -c $COOKIEJAR -b $COOKIEJAR \
        --data "user=admin&password=Secret123" \
        -X POST \
        https://$IPAHOSTNAME/ipa/session/login_password

curl -v  \
    -H referer:https://$IPAHOSTNAME/ipa  \
        -H "Content-Type:application/json" \
        -H "Accept:applicaton/json"\
        -c $COOKIEJAR -b $COOKIEJAR \
        -d  '{"method":"user_find","params":[[""],{}],"id":0}' \
        -X POST \
        https://$IPAHOSTNAME/ipa/session/json

*/

struct FreeAuth {
    server: String,
    jar: Arc<Jar>,
}

///
/// Login to the FreeIPA server using the passed username and password.
/// This returns a cookie jar that will contain the resulting authorisation
/// cookie, and which can be used for subsequent calls to the server.
///
async fn login(server: &str, user: &str, password: &str) -> Result<FreeAuth, Error> {
    let jar = Arc::new(Jar::default());

    let client = Client::builder()
        .cookie_provider(Arc::clone(&jar))
        .build()
        .context("Could not build client")?;

    let url = format!("{}/ipa/session/login_password", server);

    let result = client
        .post(&url)
        .header("Referer", format!("{}/ipa", server))
        .header("Content-Type", "application/x-www-form-urlencoded")
        .header("Accept", "text/plain")
        .body(format!("user={}&password={}", user, password))
        .send()
        .await
        .with_context(|| format!("Could not login calling URL: {}", url))?;

    match result.status() {
        status if status.is_success() => Ok(FreeAuth {
            server: server.to_string(),
            jar,
        }),
        _ => Err(Error::Login(format!(
            "Could not login to server: {}. Status: {}. Response: {:?}",
            server,
            result.status(),
            result
        ))),
    }
}

///
/// Call a post URL on the FreeIPA server described in 'auth'.
///
async fn call_post<T>(auth: &FreeAuth, payload: serde_json::Value) -> Result<T, Error>
where
    T: DeserializeOwned,
{
    tracing::info!("Calling post {}", payload);

    let url = format!("{}/ipa/session/json", &auth.server);

    let client = Client::builder()
        .cookie_provider(Arc::clone(&auth.jar))
        .build()
        .context("Could not build client")?;

    let result = client
        .post(&url)
        .header("Referer", format!("{}/ipa", &auth.server))
        .header("Content-Type", "application/json")
        .header("Accept", "application/json")
        .json(&payload)
        .send()
        .await
        .with_context(|| format!("Could not call function: {}", payload))?;

    if result.status().is_success() {
        tracing::info!("Response: {:?}", result);
        Ok(result
            .json::<T>()
            .await
            .context("Could not decode from json")?)
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

#[derive(Debug, Clone, Serialize, Deserialize)]
struct IPAResponse {
    result: serde_json::Value,
}

pub async fn connect() -> Result<(), Error> {
    let auth = login("https://ipa.demo1.freeipa.org", "admin", "Secret123").await?;

    let result = call_post::<IPAResponse>(
        &auth,
        serde_json::json!({"method":"user_find","params":[[""],{}],"id":0}),
    )
    .await?;

    tracing::info!("Result: {:?}", result);

    Ok(())
}

/// Errors

#[derive(Error, Debug)]
pub enum Error {
    #[error("{0}")]
    Any(#[from] AnyError),

    #[error("{0}")]
    Call(String),

    #[error("{0}")]
    Login(String),
}
