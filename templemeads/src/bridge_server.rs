// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use crate::bridge::run as bridge_run;
use crate::job::Job;
use anyhow::{Context, Error as AnyError, Result};
use axum::{
    extract::{Json, State},
    http::header::HeaderMap,
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
    Router,
};
use chrono::{DateTime, Duration, Utc};
use paddington::{Key, SecretKey};
use secrecy::ExposeSecret;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::net::IpAddr;
use thiserror::Error;
use tokio::net::TcpListener;
use url::{ParseError, Url};

///
/// Return the OpenPortal authorisation header for the passed datetime,
/// protocol, function and (optional) arguments, signed with the passed
/// key.
///
pub fn sign_api_call(
    key: &SecretKey,
    date: &DateTime<Utc>,
    protocol: &str,
    function: &str,
    arguments: &Option<serde_json::Value>,
) -> Result<String, anyhow::Error> {
    let date = date.format("%a, %d %b %Y %H:%M:%S GMT").to_string();

    let call_string = match arguments {
        Some(args) => format!(
            "{}\napplication/json\n{}\n{}\n{}",
            protocol, date, function, args
        ),
        None => format!("{}\napplication/json\n{}\n{}", protocol, date, function),
    };

    let signature = key.expose_secret().sign(call_string)?;
    Ok(format!("OpenPortal {}", signature))
}

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

///
/// Verify the headers for the request - this checks the API key
///
fn verify_headers(
    state: &AppState,
    headers: &HeaderMap,
    protocol: &str,
    function: &str,
    arguments: Option<serde_json::Value>,
) -> Result<(), AppError> {
    let key = match headers.get("Authorization") {
        Some(key) => key,
        None => {
            tracing::error!("No API key in headers {:?}", headers);
            return Err(AppError(
                anyhow::anyhow!("No API key in headers"),
                Some(StatusCode::UNAUTHORIZED),
            ));
        }
    }
    .to_str()
    .unwrap_or_default()
    .to_string();

    let date = match headers.get("Date") {
        Some(date) => date,
        None => {
            tracing::error!("No date in headers {:?}", headers);
            return Err(AppError(
                anyhow::anyhow!("No date in headers"),
                Some(StatusCode::UNAUTHORIZED),
            ));
        }
    }
    .to_str()
    .map_err(|e| {
        tracing::error!("Could not parse date: {:?}", e);
        AppError(
            anyhow::anyhow!("Could not parse date"),
            Some(StatusCode::UNAUTHORIZED),
        )
    })?;

    let date = DateTime::parse_from_rfc2822(date)
        .map_err(|e| {
            tracing::error!("Could not parse date: {:?}", e);
            AppError(
                anyhow::anyhow!("Could not parse date"),
                Some(StatusCode::UNAUTHORIZED),
            )
        })?
        .with_timezone(&Utc);

    // make sure that this date is within the last 5 minutes
    let now = Utc::now();

    if now - date > Duration::minutes(5) || date - now > Duration::minutes(5) {
        tracing::error!("Date is too old");
        return Err(AppError(
            anyhow::anyhow!("Date is too old"),
            Some(StatusCode::UNAUTHORIZED),
        ));
    }

    // now generate the expected key
    let expected_key = sign_api_call(&state.config.key, &date, protocol, function, &arguments)?;

    if key != expected_key {
        tracing::error!("API key does not match the expected key");
        tracing::error!("Expected: {}", expected_key);
        tracing::error!("Got: {}", key);
        tracing::error!("Signed for date: {:?}", date);
        tracing::error!("Protocol: {}", protocol);
        tracing::error!("Function: {}", function);
        tracing::error!("Arguments: {:?}", arguments);
        return Err(AppError(
            anyhow::anyhow!("API key is invalid!"),
            Some(StatusCode::UNAUTHORIZED),
        ));
    }

    Ok(())
}

//
// Shared state for the web API - simple key-value store protected
// by a tokio Mutex.
//
#[derive(Clone, Debug)]
struct AppState {
    config: Config,
    // data: Arc<Mutex<HashMap<String, String>>>, <- this is how to have shared state
}

//
// Health check endpoint for the web API
//
#[tracing::instrument(skip_all)]
async fn health(
    headers: HeaderMap,
    State(state): State<AppState>,
) -> Result<Json<serde_json::Value>, AppError> {
    verify_headers(&state, &headers, "get", "health", None)?;
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
// The 'run' endpoint for the web API. This is the main entry point
// to which commands are submitted to OpenPortal. This will return
// a JSON object that represents the Job that has been created.
//
#[tracing::instrument(skip_all)]
async fn run(
    headers: HeaderMap,
    State(state): State<AppState>,
    //Query(params): Query<HashMap<String, String>>,
    Json(payload): Json<RunRequest>,
) -> Result<Json<Job>, AppError> {
    verify_headers(
        &state,
        &headers,
        "post",
        "run",
        Some(serde_json::json!({"command": payload.command})),
    )?;

    tracing::info!("Running command: {}", payload.command);

    Ok(Json(bridge_run(&payload.command).await?))
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
        config: config.clone(),
        // data: Arc::new(Mutex::new(HashMap::new())),
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