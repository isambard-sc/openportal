// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use anyhow::{anyhow, Context, Result};

use axum::{
    extract::Json,
    handler::Handler,
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
    Router,
};
use serde::Deserialize;
use serde_json::json;

use std::sync::Arc;
use tokio::net::TcpListener;

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

#[derive(Debug)]
struct AppState {
    something: String,
}

#[tracing::instrument(skip_all)]
pub async fn health() {
    tracing::info!("Health check");
}

#[derive(Deserialize, Debug)]
struct RunRequest {
    command: String,
}

#[tracing::instrument(skip_all)]
async fn run(Json(payload): Json<RunRequest>) {
    tracing::info!("Running command: {}", payload.command);
}

#[tokio::main]
async fn main() -> Result<()> {
    // construct a subscriber that prints formatted traces to stdout
    let subscriber = tracing_subscriber::FmtSubscriber::new();
    // use that subscriber to process traces emitted after this point
    tracing::subscriber::set_global_default(subscriber)?;

    let defaults = paddington::Defaults::new(
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

    let state = Arc::new(AppState {
        something: "hello".to_string(),
    });

    let port = 3000;

    let app = Router::new()
        .route("/", get(|| async { Json(serde_json::Value::Null) }))
        .route("/health", get(health))
        .route("/run", post(run))
        .with_state(state);

    let listener =
        tokio::net::TcpListener::bind(&std::net::SocketAddr::new("::".parse()?, port)).await?;

    // spawn a new task to run the web server to listen for requests
    tokio::spawn(run_server(app, listener));

    templemeads::agent::run(defaults).await?;

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
