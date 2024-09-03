// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use anyhow::Result;
use axum::{
    extract::{Json, Query, State},
    http::header::HeaderMap,
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
    Router,
};
use paddington::args::Defaults as CoreDefaults;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::net::TcpListener;
use tokio::sync::Mutex;

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
pub async fn health() -> Result<Json<serde_json::Value>, AppError> {
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
pub struct Job {
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

///
/// Main function for the bridge application
///
/// The purpose of this application is to bridge between the user portal
/// (e.g. Waldur) and OpenPortal.
///
/// It does this by providing a "Client" agent in OpenPortal that can be
/// used to make requests over the OpenPortal protocol.
///
/// It also provides a web API that can be called by the user portal to
/// submit and get information about those requests. This API is designed
/// to be called via, e.g. the openportal Python client.
///
#[tokio::main]
async fn main() -> Result<()> {
    // start tracing
    let subscriber = tracing_subscriber::FmtSubscriber::new();
    tracing::subscriber::set_global_default(subscriber)?;

    // process command line arguments and get info about the Client
    let defaults = CoreDefaults::new(
        Some("client".to_string()),
        Some(
            "client.toml"
                .parse()
                .expect("Could not parse default config file."),
        ),
        Some("ws://localhost:8041".to_string()),
        Some("127.0.0.1".to_string()),
        Some(8041),
    );

    // create a global state object for the web API
    let state = AppState {
        data: Arc::new(Mutex::new(HashMap::new())),
    };

    // this should be configurable
    let port = 3000;

    // create the web API
    let app = Router::new()
        .route("/", get(|| async { Json(serde_json::Value::Null) }))
        .route("/health", get(health))
        .route("/run", post(run))
        .with_state(state);

    // create a TCP listener on the specified port
    let listener =
        tokio::net::TcpListener::bind(&std::net::SocketAddr::new("::".parse()?, port)).await?;

    // spawn a new task to run the web server to listen for requests
    tokio::spawn(run_server(app, listener));

    // run the Client agent
    templemeads::agent::run(defaults).await?;

    Ok(())
}

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

trait Status<T> {
    /// Add a HTTP status code to an error.
    fn status(self, status: axum::http::StatusCode) -> Result<T, AppError>;
}

impl<T> Status<T> for anyhow::Result<T> {
    fn status(self, status: axum::http::StatusCode) -> Result<T, AppError> {
        self.map_err(|e| AppError(e, Some(status)))
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
