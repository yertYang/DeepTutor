use crate::{Application, Error, Result, application::blocking, calendar as data, data_api::Scope};
use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{
        IntoResponse, Response, Sse,
        sse::{Event, KeepAlive},
    },
    routing::{get, post},
};
use futures_util::{Stream, stream};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{convert::Infallible, sync::Arc, time::Duration};
type App = Arc<Application>;
pub fn router() -> Router<App> {
    Router::new()
        .route(
            "/api/calendar/calendars",
            get(calendars).post(create_calendar),
        )
        .route(
            "/api/calendar/calendars/{id}",
            axum::routing::patch(update_calendar).delete(delete_calendar),
        )
        .route("/api/calendar/events", get(events).post(create_event))
        .route(
            "/api/calendar/events/{id}",
            axum::routing::patch(update_event).delete(delete_event),
        )
        .route("/api/calendar/search", get(search))
        .route("/api/calendar/layers", get(layers))
        .route("/api/calendar/task-blocks", get(blocks))
        .route("/api/calendar/undo/{id}", post(undo))
        .route("/api/calendar/export.ics", get(export))
        .route("/api/calendar/stream", get(sync))
        .layer(DefaultBodyLimit::max(64 * 1024))
}
async fn read(
    app: App,
    scope: Scope,
    f: impl FnOnce(&rusqlite::Connection, &str) -> Result<Value> + Send + 'static,
) -> Result<Json<Value>> {
    crate::data_api::run(app, scope, false, |_, _| Ok(()), f).await
}
async fn write(
    app: App,
    scope: Scope,
    headers: HeaderMap,
    op: &'static str,
    id: String,
    input: Value,
) -> Result<Json<Value>> {
    let expected = headers
        .get("if-match")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.trim_matches('"').parse::<i64>().ok())
        .ok_or_else(|| {
            Error::new(
                StatusCode::PRECONDITION_REQUIRED,
                "revision_required",
                "Load the calendar before saving changes.",
            )
        })?;
    let command = headers
        .get("idempotency-key")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_owned();
    crate::data_api::run(
        app,
        scope,
        true,
        |_, _| Ok(()),
        move |c, _| data::mutate(c, expected, &command, op, &id, &input),
    )
    .await
}
async fn calendars(State(app): State<App>, Query(scope): Query<Scope>) -> Result<Json<Value>> {
    read(app, scope, |c, _| data::calendars(c)).await
}
macro_rules! create_handler {
    ($name:ident,$op:literal) => {
        async fn $name(
            State(app): State<App>,
            Query(scope): Query<Scope>,
            headers: HeaderMap,
            Json(input): Json<Value>,
        ) -> Result<Json<Value>> {
            write(app, scope, headers, $op, String::new(), input).await
        }
    };
}
create_handler!(create_calendar, "calendar_create");
create_handler!(create_event, "event_create");
macro_rules! update_handler {
    ($name:ident,$op:literal) => {
        async fn $name(
            State(app): State<App>,
            Path(id): Path<String>,
            Query(scope): Query<Scope>,
            headers: HeaderMap,
            Json(input): Json<Value>,
        ) -> Result<Json<Value>> {
            write(app, scope, headers, $op, id, input).await
        }
    };
}
update_handler!(update_calendar, "calendar_update");
update_handler!(update_event, "event_update");
async fn delete_calendar(
    State(app): State<App>,
    Path(id): Path<String>,
    Query(scope): Query<Scope>,
    headers: HeaderMap,
) -> Result<Json<Value>> {
    write(app, scope, headers, "calendar_delete", id, json!({})).await
}
#[derive(Deserialize)]
struct Delete {
    #[serde(default = "all")]
    scope: String,
    occurrence: Option<String>,
}
fn all() -> String {
    "all".into()
}
async fn delete_event(
    State(app): State<App>,
    Path(id): Path<String>,
    Query(scope): Query<Scope>,
    Query(input): Query<Delete>,
    headers: HeaderMap,
) -> Result<Json<Value>> {
    write(
        app,
        scope,
        headers,
        "event_delete",
        id,
        json!({"scope":input.scope,"occurrence":input.occurrence}),
    )
    .await
}
async fn undo(
    State(app): State<App>,
    Path(id): Path<String>,
    Query(scope): Query<Scope>,
    headers: HeaderMap,
) -> Result<Json<Value>> {
    write(app, scope, headers, "undo", id, json!({})).await
}
#[derive(Deserialize)]
struct Range {
    start: String,
    end: String,
    tz: String,
}
async fn events(
    State(app): State<App>,
    Query(scope): Query<Scope>,
    Query(range): Query<Range>,
) -> Result<Json<Value>> {
    read(app, scope, move |c, _| {
        data::occurrences(c, &range.start, &range.end, &range.tz)
    })
    .await
}
async fn layers(
    State(app): State<App>,
    Query(scope): Query<Scope>,
    Query(range): Query<Range>,
) -> Result<Json<Value>> {
    read(app, scope, move |c, s| {
        data::layers(c, s, &range.start, &range.end, &range.tz)
    })
    .await
}
#[derive(Deserialize)]
struct Search {
    q: String,
}
async fn search(
    State(app): State<App>,
    Query(scope): Query<Scope>,
    Query(input): Query<Search>,
) -> Result<Json<Value>> {
    read(app, scope, move |c, _| data::search(c, &input.q)).await
}
async fn blocks(State(app): State<App>, Query(scope): Query<Scope>) -> Result<Json<Value>> {
    read(app,scope,|c,s|{
 let rows=c.prepare("SELECT json_extract(payload_json,'$.task_id'),COUNT(*) FROM native_calendar_objects WHERE kind='event' AND json_extract(payload_json,'$.task_id') IS NOT NULL AND COALESCE(json_extract(payload_json,'$.task_workspace_id'),'')=? GROUP BY json_extract(payload_json,'$.task_id')")?.query_map([s],|r|Ok((r.get::<_,String>(0)?,r.get::<_,i64>(1)?)))?.collect::<rusqlite::Result<Vec<_>>>()?;Ok(json!(rows.into_iter().map(|(k,v)|(k,json!(v))).collect::<serde_json::Map<String,Value>>() ))}).await
}
#[derive(Deserialize)]
struct Export {
    calendar_id: Option<String>,
}
async fn export(
    State(app): State<App>,
    Query(scope): Query<Scope>,
    Query(input): Query<Export>,
) -> Result<Response> {
    scope.id()?;
    let content = blocking(move || {
        let mut c = app.store.connect()?;
        let tx = c.transaction()?;
        let value = data::export(&tx, input.calendar_id.as_deref())?;
        tx.commit()?;
        Ok(value)
    })
    .await?;
    Ok((
        [
            ("content-type", "text/calendar; charset=utf-8"),
            ("content-disposition", "attachment; filename=calendar.ics"),
            ("x-content-type-options", "nosniff"),
        ],
        content,
    )
        .into_response())
}
async fn sync(
    State(app): State<App>,
    Query(scope): Query<Scope>,
) -> Result<Sse<impl Stream<Item = std::result::Result<Event, Infallible>>>> {
    scope.id()?;
    let store = app.store.clone();
    let stopping = app.shutdown_receiver();
    let events = stream::unfold(
        (store, stopping, true),
        |(store, mut stopping, first)| async move {
            if *stopping.borrow() {
                return None;
            }
            if !first {
                tokio::select! {_=stopping.changed()=>return None,_=tokio::time::sleep(Duration::from_secs(2))=>{}}
            }
            let input = store.clone();
            let revision = blocking(move || data::revision(&input.connect()?))
                .await
                .ok()?;
            Some((
                Ok(Event::default().data(json!({"revision":revision}).to_string())),
                (store, stopping, false),
            ))
        },
    );
    Ok(Sse::new(events).keep_alive(KeepAlive::new().interval(Duration::from_secs(15))))
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::StreamExt;
    use std::future::IntoFuture;
    #[tokio::test]
    async fn http_calendar_and_tasks_enforce_revisions_and_shutdown_streams() {
        let dir = tempfile::tempdir().unwrap();
        let app = Application::open(dir.path()).unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let server =
            tokio::spawn(axum::serve(listener, crate::api::router(app.clone())).into_future());
        let client = reqwest::Client::builder().no_proxy().build().unwrap();
        assert_eq!(
            client
                .get(format!(
                    "{base}/api/calendar/calendars?workspace_id=a&dt_workspace=b"
                ))
                .send()
                .await
                .unwrap()
                .status(),
            400
        );
        let input = json!({"title":"HTTP study","start":"2026-11-01T14:00:00Z","end":"2026-11-01T15:00:00Z"});
        assert_eq!(
            client
                .post(format!("{base}/api/calendar/events"))
                .json(&input)
                .send()
                .await
                .unwrap()
                .status(),
            428
        );
        let r = client
            .post(format!("{base}/api/calendar/events"))
            .header("If-Match", "\"0\"")
            .header("Idempotency-Key", "create-http")
            .json(&input)
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), 200);
        let created: Value = r.json().await.unwrap();
        let retry: Value = client
            .post(format!("{base}/api/calendar/events"))
            .header("If-Match", "\"0\"")
            .header("Idempotency-Key", "create-http")
            .json(&input)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(created, retry);
        assert_eq!(
            client
                .post(format!("{base}/api/calendar/events"))
                .header("If-Match", "0")
                .header("Idempotency-Key", "stale")
                .json(&input)
                .send()
                .await
                .unwrap()
                .status(),
            409
        );
        let task: Value = client
            .post(format!("{base}/api/task-board/cards?dt_workspace=a"))
            .header("If-Match", "1")
            .header("Idempotency-Key", "task-http")
            .json(&json!({"title":"Reading task","due_date":"2026-11-01"}))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(task["cards"][0]["title"], "Reading task");
        let other: Value = client
            .get(format!("{base}/api/task-board?dt_workspace=b"))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(other["cards"], json!([]));
        let ics = client
            .get(format!("{base}/api/calendar/export.ics"))
            .send()
            .await
            .unwrap();
        assert_eq!(
            ics.headers()["content-type"],
            "text/calendar; charset=utf-8"
        );
        assert!(ics.text().await.unwrap().contains("SUMMARY:HTTP study"));
        let mut stream = client
            .get(format!("{base}/api/calendar/stream"))
            .send()
            .await
            .unwrap()
            .bytes_stream();
        assert!(stream.next().await.unwrap().unwrap().starts_with(b"data:"));
        app.shutdown().await;
        tokio::time::timeout(Duration::from_secs(3), async {
            while stream.next().await.is_some() {}
        })
        .await
        .unwrap();
        server.abort();
    }
}
