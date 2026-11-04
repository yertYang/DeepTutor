use crate::{Application, Result, application::blocking, data_api::Scope, reading as data};
use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, Path, Query, State},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::Arc;
type App = Arc<Application>;
pub fn router() -> Router<App> {
    Router::new()
        .route("/api/reading/items", get(list).post(add))
        .route(
            "/api/reading/items/{id}",
            get(read).patch(rename).delete(delete),
        )
        .route(
            "/api/reading/items/{id}/progress",
            get(progress).put(save_progress),
        )
        .route("/api/reading/items/{id}/source", get(source))
        .route("/api/reading/items/{id}/original", get(original))
        .route("/api/reading/items/{id}/export", get(export))
        .route(
            "/api/reading/items/{id}/annotations",
            get(annotations).post(annotate),
        )
        .route(
            "/api/reading/items/{id}/annotations/{annotation}",
            axum::routing::put(edit_annotation).delete(delete_annotation),
        )
        .route(
            "/api/reading/items/{id}/annotations/{annotation}/note",
            post(clip),
        )
        .layer(DefaultBodyLimit::max(64 * 1024))
}
async fn run(
    app: App,
    scope: Scope,
    write: bool,
    f: impl FnOnce(&rusqlite::Connection, &str) -> Result<Value> + Send + 'static,
) -> Result<Json<Value>> {
    crate::data_api::run(app, scope, write, |_, _| Ok(()), f).await
}
#[derive(Deserialize, Default)]
struct Page {
    #[serde(default)]
    offset: u32,
}
async fn list(
    State(app): State<App>,
    Query(scope): Query<Scope>,
    Query(page): Query<Page>,
) -> Result<Json<Value>> {
    let settings = app.settings.clone();
    let accounts = app.accounts.clone();
    run(app, scope, false, move |c, s| {
        let mut result = data::list(c, s, page.offset)?;
        if let Some(id) = settings.account_id.as_deref() {
            let root = settings.shared.as_ref().unwrap_or(&settings);
            let policy = crate::accounts_api::policy(root, &accounts.user(id)?)?;
            if policy["reading"]["assigned_materials"].is_array() {
                if let Some(items) = result["items"].as_array_mut() {
                    items.retain(|item| {
                        policy["reading"]["material_ids"]
                            .as_array()
                            .is_some_and(|ids| ids.iter().any(|id| id == &item["id"]))
                    });
                }
            }
        }
        Ok(result)
    })
    .await
}
async fn add(
    State(app): State<App>,
    Query(scope): Query<Scope>,
    Json(input): Json<Value>,
) -> Result<Json<Value>> {
    let settings = app.settings.clone();
    let response = run(app, scope, true, move |c, s| data::add(c, s, &input)).await?;
    if let Some(id) = settings.account_id.clone() {
        let item = response.0["id"].clone();
        blocking(move || {
            let root = settings.shared.as_ref().unwrap_or(&settings);
            let _lock = root.settings_lock()?;
            let mut grant = crate::accounts::grant(root, &id)?;
            let reading = &mut grant["learning_policy"]["reading"];
            if reading["assigned_materials"].is_array() {
                if let Some(ids) = reading["material_ids"].as_array_mut() {
                    if !ids.contains(&item) {
                        ids.push(item);
                    }
                }
                crate::config::write_json(&crate::accounts::grant_path(root, &id)?, &grant)?;
            }
            Ok(())
        })
        .await?;
    }
    Ok(response)
}
macro_rules! read_handler {
    ($name:ident, $func:ident) => {
        async fn $name(
            State(app): State<App>,
            Path(id): Path<String>,
            Query(scope): Query<Scope>,
        ) -> Result<Json<Value>> {
            run(app, scope, false, move |c, s| data::$func(c, s, &id)).await
        }
    };
}
read_handler!(read, read);
read_handler!(progress, progress);
read_handler!(annotations, annotations);
macro_rules! write_handler {
    ($name:ident, $func:ident) => {
        async fn $name(
            State(app): State<App>,
            Path(id): Path<String>,
            Query(scope): Query<Scope>,
            Json(input): Json<Value>,
        ) -> Result<Json<Value>> {
            run(app, scope, true, move |c, s| data::$func(c, s, &id, &input)).await
        }
    };
}
write_handler!(rename, rename);
write_handler!(delete, delete);
write_handler!(save_progress, save_progress);
write_handler!(annotate, annotate);
macro_rules! annotation_handler {
    ($name:ident, $func:ident) => {
        async fn $name(
            State(app): State<App>,
            Path((id, a)): Path<(String, String)>,
            Query(scope): Query<Scope>,
            Json(input): Json<Value>,
        ) -> Result<Json<Value>> {
            run(app, scope, true, move |c, s| {
                data::$func(c, s, &id, &a, &input)
            })
            .await
        }
    };
}
annotation_handler!(edit_annotation, edit_annotation);
annotation_handler!(delete_annotation, delete_annotation);
annotation_handler!(clip, clip);
async fn source(
    State(app): State<App>,
    Path(id): Path<String>,
    Query(scope): Query<Scope>,
) -> Result<Json<Value>> {
    run(app, scope, false, move |c, s| {
        Ok(json!({"content":data::source(c,s,&id)?}))
    })
    .await
}
fn download(name: &str, mime: &str, bytes: Vec<u8>) -> Response {
    // RFC 5987; even non-ASCII names and quotes cannot inject response headers.
    let encoded = name
        .bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || b"-._".contains(&b) {
                char::from(b).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect::<String>();
    (
        [
            ("content-type", mime.to_string()),
            (
                "content-disposition",
                format!("attachment; filename*=UTF-8''{encoded}"),
            ),
            ("x-content-type-options", "nosniff".to_string()),
        ],
        bytes,
    )
        .into_response()
}
async fn original(
    State(app): State<App>,
    Path(id): Path<String>,
    Query(scope): Query<Scope>,
) -> Result<Response> {
    let s = scope.id()?;
    let (name, bytes) = blocking(move || {
        let mut c = app.store.connect()?;
        let tx = c.transaction()?;
        let result = data::original(&tx, &s, &id)?;
        tx.commit()?;
        Ok(result)
    })
    .await?;
    Ok(download(&name, "application/octet-stream", bytes))
}
async fn export(
    State(app): State<App>,
    Path(id): Path<String>,
    Query(scope): Query<Scope>,
) -> Result<Response> {
    let s = scope.id()?;
    let value = blocking(move || {
        let mut c = app.store.connect()?;
        let tx = c.transaction()?;
        let v = data::export(&tx, &s, &id)?;
        tx.commit()?;
        Ok(v)
    })
    .await?;
    Ok(download(
        "reading.json",
        "application/json; charset=utf-8",
        value.to_string().into_bytes(),
    ))
}
