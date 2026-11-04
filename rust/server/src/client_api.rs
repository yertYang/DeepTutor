//! HTTP transport for lightweight native-service clients. It enters the same
//! application as WebSocket and CLI; polling replays only committed events.
use crate::{Application, Error, Result, application::blocking, data_api::Scope, store::text};
use axum::{
    Json, Router,
    extract::{Path, Query, State},
    routing::post,
};
use serde_json::{Value, json};
use std::sync::Arc;
type App = Arc<Application>;
pub fn router() -> Router<App> {
    Router::new()
        .route("/api/turns", post(start))
        .route("/api/turns/{turn}/reply", post(reply))
        .route("/api/turns/{turn}/cancel", post(cancel))
        .route("/api/native/backup", post(backup))
}
async fn start(
    State(app): State<App>,
    Query(scope): Query<Scope>,
    Json(mut body): Json<Value>,
) -> Result<Json<Value>> {
    let scope = scope.id()?;
    if !body.is_object() {
        return Err(Error::invalid("Expected a turn object."));
    }
    if body.get("workspace_id").is_some_and(|v| v != &scope) {
        return Err(Error::invalid(
            "Turn workspace does not match the transport scope.",
        ));
    }
    body["workspace_id"] = json!(scope);
    if text(&body, "client_submission_id").is_empty() {
        return Err(Error::invalid(
            "HTTP turn submission requires a stable client_submission_id.",
        ));
    }
    let receipt = app.start(body).await?;
    Ok(Json(
        json!({"protocol_version":"2.0","session_id":receipt.session,"turn_id":receipt.turn,"user_message_id":receipt.user_message,"replay":receipt.replay,"capability":receipt.capability.id()}),
    ))
}
async fn reply(
    State(app): State<App>,
    Path(turn): Path<String>,
    Query(scope): Query<Scope>,
    Json(mut body): Json<Value>,
) -> Result<Json<Value>> {
    let scope = scope.id()?;
    if !body.is_object() {
        return Err(Error::invalid("Expected a reply object."));
    }
    if body.get("turn_id").is_some_and(|v| v != &turn) {
        return Err(Error::invalid("Reply turn does not match the URL."));
    }
    body["type"] = json!("submit_user_reply");
    body["protocol_version"] = json!("2.0");
    body["turn_id"] = json!(turn);
    Ok(Json(app.reply_command(body, &scope).await?))
}
async fn cancel(
    State(app): State<App>,
    Path(turn): Path<String>,
    Query(scope): Query<Scope>,
    Json(body): Json<Value>,
) -> Result<Json<Value>> {
    let scope = scope.id()?;
    let command = text(&body, "command_id");
    if command.is_empty() {
        return Err(Error::invalid("Cancellation requires a stable command_id."));
    }
    Ok(Json(app.cancel_command(&turn, &scope, command).await?))
}
async fn backup(State(app): State<App>) -> Result<Json<Value>> {
    Ok(Json(blocking(move || app.store.backup()).await?))
}
