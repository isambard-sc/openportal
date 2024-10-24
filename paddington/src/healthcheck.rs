// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use crate::Error;

use anyhow::Result;
use axum::{
    extract::Json,
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::get,
    Router,
};
use once_cell::sync::Lazy;
use serde_json::json;
use std::net::IpAddr;
use std::sync::RwLock;
use tokio::net::TcpListener;

//
// Health check endpoint for the web API
//
#[tracing::instrument(skip_all)]
async fn health() -> Result<Json<serde_json::Value>, AppError> {
    Ok(Json(json!({"status": "ok"})))
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

static IS_RUNNING: Lazy<RwLock<bool>> = Lazy::new(|| RwLock::new(false));

///
/// Spawn a small http server that responds to health checks
///
pub async fn spawn(ip: IpAddr, port: u16) -> Result<(), Error> {
    // check if the server is already running
    match IS_RUNNING.read() {
        Ok(guard) => {
            if *guard {
                // already running
                return Ok(());
            }
        }
        Err(e) => {
            // not running?
            tracing::error!("Error getting read lock: {}", e);
            return Ok(());
        }
    }

    // set the flag to indicate the server is running
    match IS_RUNNING.write() {
        Ok(mut guard) => {
            if *guard {
                // someone else set it first
                return Ok(());
            }

            *guard = true;
        }
        Err(e) => {
            // not running?
            tracing::error!("Error getting write lock: {}", e);
            return Ok(());
        }
    }

    tracing::info!("Starting health check server on {}:{}/health", ip, port);

    // create the web API
    let app = Router::new().route("/health", get(health));

    // create a TCP listener on the specified port
    let listener = tokio::net::TcpListener::bind(&std::net::SocketAddr::new(ip, port)).await?;

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
