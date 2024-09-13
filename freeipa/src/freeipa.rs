// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use anyhow::Result;
use anyhow::{Context, Error as AnyError};
use reqwest::{cookie::Jar, header::HeaderMap, header::HeaderValue, Client};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::json;
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
        Err(Error::CallError(format!(
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

async fn call_post(url: &str) -> Result<String, Error> {
    let server = "https://ipa.demo1.freeipa.org/ipa";

    tracing::info!("Calling post {}", url);

    let url = format!("{}/{}", server, url);

    let jar = Arc::new(Jar::default());

    let mut headers = HeaderMap::new();
    headers.insert(
        "Content-Type",
        HeaderValue::from_static("application/x-www-form-urlencoded"),
    );
    headers.insert("Accept", HeaderValue::from_static("text/plain"));
    headers.insert("Referer", HeaderValue::from_static(server));

    let client = Client::builder()
        .default_headers(headers)
        .cookie_provider(Arc::clone(&jar))
        .build()
        .context("Could not build client")?;

    let result = client
        .post(&url)
        .body("user=admin&password=Secret123")
        .send()
        .await
        .with_context(|| format!("Could not call function: {}", url))?;

    tracing::info!("Response: {:?}", result);

    tracing::info!("Cookies: {:?}", jar);

    if result.status().is_success() {
        let text = result.text().await.context("Could not get response?")?;
        tracing::info!("Response: {:?}", text);
    } else {
        return Err(Error::CallError(format!(
            "Could not get response for function: {}. Status: {}. Response: {:?}",
            url,
            result.status(),
            result
        )));
    }

    // now make an API call
    let mut headers = HeaderMap::new();
    headers.insert("Content-Type", HeaderValue::from_static("application/json"));
    headers.insert("Accept", HeaderValue::from_static("application/json"));
    headers.insert("Referer", HeaderValue::from_static(server));

    let client = Client::builder()
        .default_headers(headers)
        .cookie_provider(Arc::clone(&jar))
        .build()
        .context("Could not build client")?;

    let url = format!("{}/session/json", server);
    tracing::info!("Calling post {}", url);

    let json_data = "{\"method\":\"user_find\",\"params\":[[\"\"],{}],\"id\":0}";

    // turn the above string into a serde_json value
    let json_data: serde_json::Value =
        serde_json::from_str(json_data).context("Could not parse json")?;

    let result = client
        .post(&url)
        .json(&json_data)
        .send()
        .await
        .with_context(|| format!("Could not call function: {}", url))?;

    tracing::info!("Response: {:?}", result);

    if result.status().is_success() {
        Ok(result.text().await.context("Could not get response?")?)
    } else {
        tracing::error!(
            "Could not get response for function: {}. Status: {}. Response: {:?}",
            url,
            result.status(),
            result
        );
        Err(Error::CallError(format!(
            "Could not get response for function: {}. Status: {}. Response: {:?}",
            url,
            result.status(),
            result
        )))
    }
}

pub async fn connect() -> Result<(), Error> {
    let result = call_post("session/login_password").await?;

    tracing::info!("Result: {:?}", result);

    Ok(())
}

/// Errors

#[derive(Error, Debug)]
pub enum Error {
    #[error("{0}")]
    AnyError(#[from] AnyError),

    #[error("{0}")]
    IOError(#[from] std::io::Error),

    #[error("{0}")]
    CallError(String),

    #[error("{0}")]
    InvalidConfig(String),

    #[error("{0}")]
    LockError(String),

    #[error("Unknown error")]
    Unknown,
}
