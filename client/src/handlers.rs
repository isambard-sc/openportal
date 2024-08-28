use anyhow::{anyhow, Context, Result};

use axum::{
    async_trait,
    extract::{FromRequestParts, Query, State},
    http::{request::Parts, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
    Json, RequestPartsExt as _, Router,
};
use axum_extra::{
    headers::{authorization::Bearer, Authorization},
    TypedHeader,
};

use serde_json::json;
use tokio::net::TcpListener;

#[tracing::instrument(skip_all)]
pub async fn health() -> Result<Json<serde_json::Value>, HandlerError> {
    Ok(Json(json!({})))
}

#[tracing::instrument(skip_all)]
pub async fn list_accounts() -> Result<Json<serde_json::Value>, HandlerError> {
    Ok(Json(json!({})))
}

#[tracing::instrument(skip_all)]
pub async fn get_account() -> Result<Json<serde_json::Value>, HandlerError> {
    Ok(Json(json!({})))
}

#[tracing::instrument(skip_all)]
pub async fn create_account() -> Result<Json<serde_json::Value>, HandlerError> {
    Ok(Json(json!({})))
}

#[tracing::instrument(skip_all)]
pub async fn delete_account() -> Result<Json<serde_json::Value>, HandlerError> {
    Ok(Json(json!({})))
}

#[tracing::instrument(skip_all)]
pub async fn delete_all_users_from_account() -> Result<Json<serde_json::Value>, HandlerError> {
    Ok(Json(json!({})))
}

#[tracing::instrument(skip_all)]
pub async fn account_has_users() -> Result<Json<serde_json::Value>, HandlerError> {
    Ok(Json(json!({})))
}

#[tracing::instrument(skip_all)]
pub async fn set_resource_limits() -> Result<Json<serde_json::Value>, HandlerError> {
    Ok(Json(json!({})))
}

#[tracing::instrument(skip_all)]
pub async fn get_association() -> Result<Json<serde_json::Value>, HandlerError> {
    Ok(Json(json!({})))
}

#[tracing::instrument(skip_all)]
pub async fn create_association() -> Result<Json<serde_json::Value>, HandlerError> {
    Ok(Json(json!({})))
}

#[tracing::instrument(skip_all)]
pub async fn delete_association() -> Result<Json<serde_json::Value>, HandlerError> {
    Ok(Json(json!({})))
}

#[tracing::instrument(skip_all)]
pub async fn get_resource_limits() -> Result<Json<serde_json::Value>, HandlerError> {
    Ok(Json(json!({})))
}

#[tracing::instrument(skip_all)]
pub async fn list_account_users() -> Result<Json<serde_json::Value>, HandlerError> {
    Ok(Json(json!({})))
}

// Errors

#[derive(Debug)]
pub struct HandlerError(anyhow::Error, Option<axum::http::StatusCode>);

impl IntoResponse for HandlerError {
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
    fn status(self, status: axum::http::StatusCode) -> Result<T, HandlerError>;
}

impl<T> Status<T> for anyhow::Result<T> {
    fn status(self, status: axum::http::StatusCode) -> Result<T, HandlerError> {
        self.map_err(|e| HandlerError(e, Some(status)))
    }
}

impl<E> From<E> for HandlerError
where
    E: Into<anyhow::Error>,
{
    fn from(err: E) -> Self {
        Self(err.into(), None)
    }
}
