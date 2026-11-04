use crate::{Application, Result, application::blocking, workspaces};
use axum::{
    Json, Router,
    extract::{Path, State},
    routing::get,
};
use serde_json::Value;
use std::sync::Arc;
type App = Arc<Application>;
pub fn router() -> Router<App> {
    Router::new()
        .route(
            "/api/settings/workspace/registrations",
            get(list).post(create),
        )
        .route(
            "/api/settings/workspace/registrations/{id}",
            axum::routing::patch(update),
        )
}
async fn list(State(app): State<App>) -> Result<Json<Value>> {
    Ok(Json(
        blocking(move || workspaces::list(&app.settings)).await?,
    ))
}
async fn create(State(app): State<App>, Json(input): Json<Value>) -> Result<Json<Value>> {
    Ok(Json(
        blocking(move || workspaces::save(&app.settings, None, &input)).await?,
    ))
}
async fn update(
    State(app): State<App>,
    Path(id): Path<String>,
    Json(input): Json<Value>,
) -> Result<Json<Value>> {
    Ok(Json(
        blocking(move || {
            workspaces::save(
                &app.settings,
                Some(if id == "__general__" { "" } else { &id }),
                &input,
            )
        })
        .await?,
    ))
}
