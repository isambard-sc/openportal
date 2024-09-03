// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use anyhow::{Context, Error as AnyError, Result};
use axum::{
    extract::{Json, Query, State},
    http::header::HeaderMap,
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
    Router,
};
use paddington::{Key, SecretKey};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::Arc;
use thiserror::Error;
use tokio::net::TcpListener;
use tokio::sync::Mutex;
use url::{ParseError, Url};

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Config {
    pub url: Url,
    pub ip: IpAddr,
    pub port: u16,
    pub key: SecretKey,
}

fn create_webserver_url(url: &str) -> Result<Url, Error> {
    let url = url
        .parse::<Url>()
        .with_context(|| format!("Could not parse URL: {}", url))?;

    let scheme = match url.scheme() {
        "http" => "http",
        "https" => "https",
        _ => "https",
    };

    let host = url.host_str().unwrap_or("localhost");
    let port = url.port().unwrap_or(3000);
    let path = url.path();

    Ok(format!("{}://{}:{}{}", scheme, host, port, path).parse::<Url>()?)
}

impl Config {
    pub fn parse(url: &str, ip: IpAddr, port: u16) -> Self {
        Self {
            url: create_webserver_url(url).unwrap_or_else(|e| {
                tracing::error!(
                    "Could not parse URL: {} because {}. Using http://localhost:3000 instead.",
                    e,
                    url
                );
                #[allow(clippy::unwrap_used)]
                "http://localhost:3000".parse().unwrap()
            }),
            ip,
            port,
            key: Key::generate(),
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Defaults {
    pub url: String,
    pub ip: String,
    pub port: u16,
}

impl Defaults {
    pub fn parse(url: Option<String>, ip: Option<String>, port: Option<u16>) -> Self {
        Self {
            url: url.unwrap_or("http://localhost:3000".to_owned()),
            ip: ip.unwrap_or("127.0.0.1".to_owned()),
            port: port.unwrap_or(8042),
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Invite {
    pub url: Url,
    pub key: SecretKey,
}

impl Invite {
    pub fn parse(url: &Url, key: &SecretKey) -> Self {
        Self {
            url: url.clone(),
            key: key.clone(),
        }
    }
}

//
// Shared state for the web API - simple key-value store protected
// by a tokio Mutex.
//
#[derive(Clone, Debug)]
struct AppState {
    data: Arc<Mutex<HashMap<String, String>>>,
}

//
// Health check endpoint for the web API
//
#[tracing::instrument(skip_all)]
async fn health() -> Result<Json<serde_json::Value>, AppError> {
    tracing::info!("Health check");
    Ok(Json(json!({"status": "ok"})))
}

//
// Struct to represent the requests to the 'run' endpoint
//
#[derive(Deserialize, Debug)]
struct RunRequest {
    command: String,
}

//
// Struct representing the Job that is created when a command is
// run - this is returned to the caller of the 'run' endpoint,
// and can be used as a future to get the results of the command.
//
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Job {
    pub id: String,
    pub status: String,
}

//
// The 'run' endpoint for the web API. This is the main entry point
// to which commands are submitted to OpenPortal. This will return
// a JSON object that represents the Job that has been created.
//
#[tracing::instrument(skip_all)]
async fn run(
    headers: HeaderMap,
    State(state): State<AppState>,
    Query(params): Query<HashMap<String, String>>,
    Json(payload): Json<RunRequest>,
) -> Result<Json<Job>, AppError> {
    tracing::info!("Running command: {}", payload.command);
    tracing::info!("Params: {:?}", params);
    tracing::info!("Headers: {:?}", headers);
    tracing::info!("State: {:?}", state);

    let mut data = state.data.lock().await;

    data.insert("command".to_string(), payload.command);

    Ok(Json(Job {
        id: "1234".to_string(),
        status: "running".to_string(),
    }))
}

/// Functions for the Bridge server

///
/// Function spawned to run the API server in a background thread
///
async fn run_server(app: Router, listener: TcpListener) -> Result<()> {
    match axum::serve(listener, app).await {
        Ok(_) => {
            tracing::info!("Server ran successfully");
        }
        Err(e) => {
            tracing::error!("Error starting server: {}", e);
        }
    }

    Ok(())
}

pub async fn spawn(config: Config) -> Result<(), Error> {
    // create a global state object for the web API
    let state = AppState {
        data: Arc::new(Mutex::new(HashMap::new())),
    };

    // create the web API
    let app = Router::new()
        .route("/", get(|| async { Json(serde_json::Value::Null) }))
        .route("/health", get(health))
        .route("/run", post(run))
        .with_state(state);

    // create a TCP listener on the specified port
    let listener =
        tokio::net::TcpListener::bind(&std::net::SocketAddr::new(config.ip, config.port)).await?;

    // spawn a new task to run the web server to listen for requests
    tokio::spawn(run_server(app, listener));

    Ok(())
}
// Errors

#[derive(Debug)]
struct AppError(anyhow::Error, Option<axum::http::StatusCode>);

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        (
            self.1.unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
            Json(json!({"message":format!("Something went wrong: {:?}", self.0)})),
        )
            .into_response()
    }
}

impl<E> From<E> for AppError
where
    E: Into<anyhow::Error>,
{
    fn from(err: E) -> Self {
        Self(err.into(), None)
    }
}

/// Errors

#[derive(Error, Debug)]
pub enum Error {
    #[error("{0}")]
    IO(#[from] std::io::Error),

    #[error("{0}")]
    Any(#[from] AnyError),

    #[error("{0}")]
    URLParse(#[from] ParseError),

    #[error("{0}")]
    Serde(#[from] serde_json::Error),
}
