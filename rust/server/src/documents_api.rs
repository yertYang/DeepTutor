use crate::{Application, Result, data_api::Scope, documents};
use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, Multipart, Path, Query, State},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::Arc;
type App = Arc<Application>;

pub fn router() -> Router<App> {
    Router::new()
        .route("/api/documents", get(list).post(create))
        .route(
            "/api/documents/import/docx",
            post(import_docx).layer(DefaultBodyLimit::max(
                crate::document_parser::MAX_UPLOAD + 65536,
            )),
        )
        .route("/api/documents/{id}", get(read).put(update).delete(delete))
        .route("/api/documents/{id}/backlinks", get(backlinks))
        .route("/api/documents/{id}/versions", get(versions))
        .route(
            "/api/documents/{id}/versions/{revision}/restore",
            post(restore),
        )
        .route("/api/documents/{id}/export", get(export))
}
async fn import_docx(
    State(app): State<App>,
    Query(scope): Query<Scope>,
    mut multipart: Multipart,
) -> Result<Json<Value>> {
    let mut file = None;
    let mut submission = None;
    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|_| crate::Error::invalid("Invalid document upload."))?
    {
        match field.name().unwrap_or("") {
            "file" if file.is_none() => {
                let name = field.file_name().unwrap_or("").to_owned();
                let bytes = field
                    .bytes()
                    .await
                    .map_err(|_| crate::Error::invalid("Invalid document upload."))?;
                if bytes.len() > crate::document_parser::MAX_UPLOAD {
                    return Err(crate::Error::invalid("Upload exceeds 16 MiB."));
                }
                file = Some((name, bytes));
            }
            "client_submission_id" if submission.is_none() => {
                submission = Some(
                    field
                        .text()
                        .await
                        .map_err(|_| crate::Error::invalid("Invalid import submission ID."))?,
                );
            }
            _ => {
                return Err(crate::Error::invalid(
                    "Expected one DOCX file and an import submission ID.",
                ));
            }
        }
    }
    let (name, bytes) = file.ok_or_else(|| crate::Error::invalid("A DOCX file is required."))?;
    let submission =
        submission.ok_or_else(|| crate::Error::invalid("An import submission ID is required."))?;
    run(app, scope, true, move |c, s| {
        documents::import_docx(c, s, &name, &bytes, &submission)
    })
    .await
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
        |app, scope| documents::bootstrap(&app.settings, &app.store, scope),
        f,
    )
    .await
}
async fn list(State(app): State<App>, Query(scope): Query<Scope>) -> Result<Json<Value>> {
    run(app, scope, false, |c, s| {
        Ok(json!({"documents":documents::list(c,s)?}))
    })
    .await
}
async fn create(
    State(app): State<App>,
    Query(scope): Query<Scope>,
    Json(body): Json<Value>,
) -> Result<Json<Value>> {
    run(app, scope, true, move |c, s| documents::create(c, s, &body)).await
}
async fn read(
    State(app): State<App>,
    Query(scope): Query<Scope>,
    Path(id): Path<String>,
) -> Result<Json<Value>> {
    run(app, scope, false, move |c, s| documents::read(c, s, &id)).await
}
async fn update(
    State(app): State<App>,
    Query(scope): Query<Scope>,
    Path(id): Path<String>,
    Json(body): Json<Value>,
) -> Result<Json<Value>> {
    run(app, scope, true, move |c, s| {
        documents::update(c, s, &id, &body)
    })
    .await
}
async fn delete(
    State(app): State<App>,
    Query(scope): Query<Scope>,
    Path(id): Path<String>,
) -> Result<Json<Value>> {
    run(app, scope, true, move |c, s| documents::delete(c, s, &id)).await
}
async fn backlinks(
    State(app): State<App>,
    Query(scope): Query<Scope>,
    Path(id): Path<String>,
) -> Result<Json<Value>> {
    run(app, scope, false, move |c, s| {
        documents::backlinks(c, s, &id)
    })
    .await
}
async fn versions(
    State(app): State<App>,
    Query(scope): Query<Scope>,
    Path(id): Path<String>,
) -> Result<Json<Value>> {
    run(app, scope, false, move |c, s| {
        documents::versions(c, s, &id)
    })
    .await
}
async fn restore(
    State(app): State<App>,
    Query(scope): Query<Scope>,
    Path((id, revision)): Path<(String, i64)>,
    Json(body): Json<Value>,
) -> Result<Json<Value>> {
    run(app, scope, true, move |c, s| {
        documents::restore(c, s, &id, revision, &body)
    })
    .await
}
#[derive(Deserialize)]
struct Export {
    #[serde(default)]
    format: String,
}
async fn export(
    State(app): State<App>,
    Query(scope): Query<Scope>,
    Query(options): Query<Export>,
    Path(id): Path<String>,
) -> Result<Response> {
    if !["", "md", "json"].contains(&options.format.as_str()) {
        return Err(crate::Error::invalid("Choose md or json."));
    }
    let Json(page) = run(app, scope, false, move |c, s| documents::read(c, s, &id)).await?;
    let (body, kind, ext) = if options.format == "json" {
        (page.to_string(), "application/json", "json")
    } else {
        (
            crate::store::text(&page, "content").to_string(),
            "text/markdown; charset=utf-8",
            "md",
        )
    };
    Ok((
        [
            ("content-type", kind.to_string()),
            (
                "content-disposition",
                format!(
                    "attachment; filename=\"page-{}.{}\"",
                    crate::store::text(&page, "id"),
                    ext
                ),
            ),
        ],
        body,
    )
        .into_response())
}
