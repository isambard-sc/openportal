// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use anyhow::{anyhow, Context, Result};

use axum::{
    async_trait,
    extract::{FromRequestParts, Query, State},
    http::{request::Parts, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
    routing::put,
    Json, RequestPartsExt as _, Router,
};
use axum_extra::{
    headers::{authorization::Bearer, Authorization},
    TypedHeader,
};

use serde_json::json;
use tokio::net::TcpListener;

mod handlers;

use crate::handlers::{
    account_has_users, create_account, create_association, delete_account,
    delete_all_users_from_account, delete_association, get_account, get_association,
    get_resource_limits, health, list_account_users, list_accounts, set_resource_limits,
};

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

    let port = 3000;

    let app = Router::new()
        .route("/", get(|| async { Json(serde_json::Value::Null) }))
        .route("/health", get(health))
        .route("/list_accounts", get(list_accounts))
        .route("/get_account", get(get_account))
        .route("/create_account", put(create_account))
        .route("/delete_account", put(delete_account))
        .route(
            "/delete_all_users_from_account",
            put(delete_all_users_from_account),
        )
        .route("/account_has_users", get(account_has_users))
        .route("/set_resource_limits", put(set_resource_limits))
        .route("/get_association", get(get_association))
        .route("/create_association", put(create_association))
        .route("/delete_association", put(delete_association))
        .route("/get_resource_limits", get(get_resource_limits))
        .route("/list_account_users", get(list_account_users));

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
