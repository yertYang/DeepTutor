use crate::{Application, Result, data_api::Scope, study};
use axum::{
    Json, Router,
    extract::{Path, Query, State},
    routing::{get, post},
};
use serde_json::Value;
use std::sync::Arc;
type App = Arc<Application>;
pub fn router() -> Router<App> {
    Router::new()
        .route("/api/dashboard/learning-index", get(dashboard))
        .route("/api/study", get(list))
        .route("/api/study/progress", get(progress))
        .route("/api/study/{id}", get(read))
        .route("/api/study/{id}/attempts", post(attempt))
        .route("/api/study/{id}/note", post(note))
        .route("/api/study/{id}/apply", post(apply))
}
async fn dashboard(State(app): State<App>, Query(scope): Query<Scope>) -> Result<Json<Value>> {
    crate::data_api::run(app, scope, false, |_, _| Ok(()), study::dashboard).await
}
async fn list(State(app): State<App>, Query(scope): Query<Scope>) -> Result<Json<Value>> {
    crate::data_api::run(app, scope, false, |_, _| Ok(()), study::list).await
}
async fn progress(State(app): State<App>, Query(scope): Query<Scope>) -> Result<Json<Value>> {
    crate::data_api::run(app, scope, false, |_, _| Ok(()), study::progress).await
}
async fn read(
    State(app): State<App>,
    Query(scope): Query<Scope>,
    Path(id): Path<String>,
) -> Result<Json<Value>> {
    crate::data_api::run(
        app,
        scope,
        false,
        |_, _| Ok(()),
        move |c, s| study::read(c, s, &id),
    )
    .await
}
async fn attempt(
    State(app): State<App>,
    Query(scope): Query<Scope>,
    Path(id): Path<String>,
    Json(input): Json<Value>,
) -> Result<Json<Value>> {
    crate::data_api::run(
        app,
        scope,
        true,
        |_, _| Ok(()),
        move |c, s| study::attempt(c, s, &id, &input),
    )
    .await
}
async fn note(
    State(app): State<App>,
    Query(scope): Query<Scope>,
    Path(id): Path<String>,
    Json(input): Json<Value>,
) -> Result<Json<Value>> {
    crate::data_api::run(
        app,
        scope,
        true,
        |app, scope| crate::documents::bootstrap(&app.settings, &app.store, scope),
        move |c, s| study::note(c, s, &id, &input),
    )
    .await
}
async fn apply(
    State(app): State<App>,
    Query(scope): Query<Scope>,
    Path(id): Path<String>,
    Json(input): Json<Value>,
) -> Result<Json<Value>> {
    crate::data_api::run(
        app,
        scope,
        true,
        |_, _| Ok(()),
        move |c, s| study::apply_proposal(c, s, &id, &input),
    )
    .await
}
