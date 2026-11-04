use crate::{
    Application, Error, Result, application::blocking, calendar, data_api::Scope, task_board,
};
use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{
        Sse,
        sse::{Event, KeepAlive},
    },
    routing::{get, patch, post, put},
};
use futures_util::{Stream, stream};
use serde_json::{Value, json};
use std::{convert::Infallible, sync::Arc, time::Duration};
type App = Arc<Application>;
pub fn router() -> Router<App> {
    Router::new()
        .route("/api/task-board", get(board))
        .route("/api/task-board/cards", post(create))
        .route("/api/task-board/cards/{id}", patch(update))
        .route("/api/task-board/colors", put(colors))
        .route("/api/task-board/sessions/{id}", put(links))
        .route("/api/task-board/sessions/{id}/status", patch(status))
        .route("/api/task-board/events", get(events))
        .layer(DefaultBodyLimit::max(64 * 1024))
}
async fn board(State(app): State<App>, Query(scope): Query<Scope>) -> Result<Json<Value>> {
    crate::data_api::run(app, scope, false, |_, _| Ok(()), task_board::board).await
}
async fn change(
    app: App,
    scope: Scope,
    h: HeaderMap,
    action: &'static str,
    id: String,
    input: Value,
) -> Result<Json<Value>> {
    let revision = h
        .get("if-match")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.trim_matches('"').parse::<i64>().ok())
        .ok_or_else(|| {
            Error::new(
                StatusCode::PRECONDITION_REQUIRED,
                "revision_required",
                "Load tasks before saving changes.",
            )
        })?;
    let command = h
        .get("idempotency-key")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_owned();
    crate::data_api::run(
        app,
        scope,
        true,
        |_, _| Ok(()),
        move |c, s| {
            calendar::mutate(
                c,
                revision,
                &command,
                "tasks",
                s,
                &json!({"action":action,"id":id,"changes":input}),
            )
        },
    )
    .await
}
async fn create(
    State(app): State<App>,
    Query(scope): Query<Scope>,
    h: HeaderMap,
    Json(input): Json<Value>,
) -> Result<Json<Value>> {
    change(app, scope, h, "create", String::new(), input).await
}
async fn colors(
    State(app): State<App>,
    Query(scope): Query<Scope>,
    h: HeaderMap,
    Json(input): Json<Value>,
) -> Result<Json<Value>> {
    change(app, scope, h, "colors", String::new(), input).await
}
macro_rules! handler {
    ($name:ident,$action:literal) => {
        async fn $name(
            State(app): State<App>,
            Query(scope): Query<Scope>,
            Path(id): Path<String>,
            h: HeaderMap,
            Json(input): Json<Value>,
        ) -> Result<Json<Value>> {
            change(app, scope, h, $action, id, input).await
        }
    };
}
handler!(update, "update");
handler!(links, "links");
handler!(status, "status");
async fn events(
    State(app): State<App>,
    Query(scope): Query<Scope>,
) -> Result<Sse<impl Stream<Item = std::result::Result<Event, Infallible>>>> {
    let scope = scope.id()?;
    let store = app.store.clone();
    let stopping = app.shutdown_receiver();
    let events = stream::unfold(
        (store, stopping, scope, true),
        |(store, mut stopping, scope, first)| async move {
            if *stopping.borrow() {
                return None;
            }
            if !first {
                tokio::select! {_=stopping.changed()=>return None,_=tokio::time::sleep(Duration::from_secs(2))=>{}}
            }
            let db = store.clone();
            let s = scope.clone();
            let board = blocking(move || {
                let mut c = db.connect()?;
                let tx = c.transaction()?;
                let b = task_board::board(&tx, &s)?;
                tx.commit()?;
                Ok(b)
            })
            .await
            .ok()?;
            Some((
                Ok(Event::default().data(board.to_string())),
                (store, stopping, scope, false),
            ))
        },
    );
    Ok(Sse::new(events).keep_alive(KeepAlive::new().interval(Duration::from_secs(15))))
}
