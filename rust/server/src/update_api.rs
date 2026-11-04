//! Forward admin update operations to the private deployment controller.
use crate::{Application, Error, Result};
use axum::{
    Json, Router,
    routing::{get, post},
};
use reqwest::Method;
use serde_json::{Value, json};
use std::{sync::Arc, time::Duration};

pub(crate) fn router() -> Router<Arc<Application>> {
    Router::new()
        .route("/api/system/update", get(status).post(update))
        .route("/api/system/update/check", post(check))
        .route("/api/system/update/job", get(job))
        .route("/api/system/update/settings", axum::routing::put(settings))
}

async fn forward(method: Method, path: &str, body: Option<Value>) -> Result<Json<Value>> {
    let url = std::env::var("DEEPTUTOR_UPDATER_URL")
        .map_err(|_| Error::unsupported("Server code updates require compose.server.yaml."))?;
    let token = std::env::var("DEEPTUTOR_UPDATE_TOKEN")
        .map_err(|_| Error::conflict("Server update token is not configured."))?;
    let client = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(120))
        .build()
        .map_err(|_| Error::conflict("Unable to create update client."))?;
    let mut request = client
        .request(method, format!("{}{path}", url.trim_end_matches('/')))
        .bearer_auth(token);
    if let Some(body) = body {
        request = request.json(&body);
    }
    let response = request.send().await.map_err(|_| {
        Error::provider("Server updater is unavailable. Check the updater container.")
    })?;
    let status = response.status();
    let payload: Value = response
        .json()
        .await
        .map_err(|_| Error::provider("Invalid server updater response."))?;
    if !status.is_success() {
        return Err(Error::new(
            status,
            "update_error",
            payload["detail"]
                .as_str()
                .unwrap_or("Server update failed."),
        ));
    }
    Ok(Json(payload))
}
async fn status() -> Result<Json<Value>> {
    forward(Method::GET, "/status", None).await
}
async fn check() -> Result<Json<Value>> {
    forward(Method::POST, "/check", None).await
}
async fn job() -> Result<Json<Value>> {
    forward(Method::GET, "/job", None).await
}
async fn settings(Json(body): Json<Value>) -> Result<Json<Value>> {
    if !body["enabled"].is_boolean() {
        return Err(Error::invalid("enabled must be a boolean."));
    }
    forward(
        Method::PUT,
        "/settings",
        Some(json!({"enabled":body["enabled"]})),
    )
    .await
}
async fn update(Json(body): Json<Value>) -> Result<Json<Value>> {
    if body["confirmation"] != "update-and-restart" {
        return Err(Error::invalid("Update confirmation is required."));
    }
    forward(Method::POST, "/update", Some(body)).await
}
