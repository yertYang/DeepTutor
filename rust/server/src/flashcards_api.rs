use crate::{Application, Result, data_api::Scope, flashcards};
use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, Path, Query, State},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde_json::Value;
use std::sync::Arc;
type App = Arc<Application>;
pub fn router() -> Router<App> {
    Router::new()
        .route("/api/documents/review/due", get(due))
        .route("/api/documents/flashcards/export", get(export_all))
        .route("/api/documents/{id}/flashcards", get(deck).put(update))
        .route("/api/documents/{id}/review", post(review))
        .route("/api/documents/{id}/flashcards/export", get(export_page))
        .layer(DefaultBodyLimit::max(2 * 1024 * 1024))
}
async fn run(
    app: App,
    scope: Scope,
    write: bool,
    f: impl FnOnce(&rusqlite::Connection, &str) -> Result<Value> + Send + 'static,
) -> Result<Json<Value>> {
    crate::data_api::run(
        app,
        scope,
        write,
        |app, s| crate::documents::bootstrap(&app.settings, &app.store, s),
        f,
    )
    .await
}
async fn due(State(app): State<App>, Query(scope): Query<Scope>) -> Result<Json<Value>> {
    run(app, scope, false, flashcards::due).await
}
async fn deck(
    State(app): State<App>,
    Query(scope): Query<Scope>,
    Path(id): Path<String>,
) -> Result<Json<Value>> {
    run(app, scope, false, move |c, s| flashcards::deck(c, s, &id)).await
}
macro_rules! mutation {
    ($name:ident,$review:literal) => {
        async fn $name(
            State(app): State<App>,
            Query(scope): Query<Scope>,
            Path(id): Path<String>,
            Json(input): Json<Value>,
        ) -> Result<Json<Value>> {
            run(app, scope, true, move |c, s| {
                flashcards::mutate(c, s, &id, $review, &input)
            })
            .await
        }
    };
}
mutation!(update, false);
mutation!(review, true);
async fn download(app: App, scope: Scope, page: Option<String>) -> Result<Response> {
    let s = scope.id()?;
    let body = crate::application::blocking(move || {
        crate::documents::bootstrap(&app.settings, &app.store, &s)?;
        let mut c = app.store.connect()?;
        let tx = c.transaction()?;
        let v = flashcards::export(&tx, &s, page.as_deref())?;
        tx.commit()?;
        Ok(v)
    })
    .await?;
    Ok((
        [
            ("content-type", "text/plain; charset=utf-8"),
            (
                "content-disposition",
                "attachment; filename=lorevia-flashcards.txt",
            ),
            ("x-content-type-options", "nosniff"),
        ],
        body,
    )
        .into_response())
}
async fn export_all(State(app): State<App>, Query(scope): Query<Scope>) -> Result<Response> {
    download(app, scope, None).await
}
async fn export_page(
    State(app): State<App>,
    Query(scope): Query<Scope>,
    Path(id): Path<String>,
) -> Result<Response> {
    download(app, scope, Some(id)).await
}
