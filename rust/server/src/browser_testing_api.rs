use crate::{
    Application, Error, Result,
    application::blocking,
    browser_testing as data,
    data_api::Scope,
    store::{id, now, text},
};
use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, Multipart, Path, Query, State},
    http::HeaderMap,
    response::{IntoResponse, Response},
    routing::{get, post},
};
use rusqlite::{OptionalExtension, TransactionBehavior, params};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::sync::Arc;
use tokio::sync::watch;
type App = Arc<Application>;
pub fn router() -> Router<App> {
    Router::new()
        .route(
            "/api/browser-testing/sources",
            get(source_list)
                .post(import_sources)
                .layer(DefaultBodyLimit::max(65 * 1024 * 1024)),
        )
        .route("/api/browser-testing/sources/{id}", get(source_read))
        .route(
            "/api/browser-testing/sources/{id}/original",
            get(source_original),
        )
        .route("/api/browser-testing/exports/{id}", get(workbook_download))
        .route("/api/browser-testing", get(state))
        .route("/api/browser-testing/config", axum::routing::put(config))
        .route("/api/browser-testing/cases", post(create))
        .route(
            "/api/browser-testing/cases/{id}",
            get(case).put(update).delete(delete),
        )
        .route("/api/browser-testing/cases/{id}/export", get(export))
        .route("/api/browser-testing/runs", post(start))
        .route("/api/browser-testing/runs/{id}", get(report))
        .route("/api/browser-testing/runs/{id}/cancel", post(cancel))
        .route("/api/browser-testing/artifacts/{id}", get(artifact))
}
async fn state(State(app): State<App>, Query(scope): Query<Scope>) -> Result<Json<Value>> {
    crate::data_api::run(app,scope,false,|_,_|Ok(()),|c,s|Ok(json!({"config":data::config(c,s)?,"cases":data::cases(c,s)?,"runs":data::reports(c,s)?,"sources":data::sources::list(c,s)?,"exports":data::sources::exports(c,s)?,"engine_available":data::worker_path().is_file()}))).await
}
async fn source_list(State(app): State<App>, Query(scope): Query<Scope>) -> Result<Json<Value>> {
    crate::data_api::run(
        app,
        scope,
        false,
        |_, _| Ok(()),
        |c, s| Ok(json!({"sources":data::sources::list(c,s)?})),
    )
    .await
}
async fn source_read(
    State(app): State<App>,
    Query(scope): Query<Scope>,
    Path(id): Path<String>,
) -> Result<Json<Value>> {
    crate::data_api::run(
        app,
        scope,
        false,
        |_, _| Ok(()),
        move |c, s| data::sources::read(c, s, &id, 0, 200),
    )
    .await
}
async fn import_sources(
    State(app): State<App>,
    Query(scope): Query<Scope>,
    mut multipart: Multipart,
) -> Result<Json<Value>> {
    let mut documents = Vec::new();
    let mut total = 0;
    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|_| Error::invalid("Invalid test input upload."))?
    {
        let name = field
            .file_name()
            .ok_or_else(|| Error::invalid("Select files to import."))?
            .rsplit(['/', '\\'])
            .next()
            .unwrap_or("")
            .to_owned();
        if documents.len() >= 32 {
            return Err(Error::invalid("Import at most 32 files per batch."));
        }
        let bytes = field
            .bytes()
            .await
            .map_err(|_| Error::invalid("Invalid test input upload."))?
            .to_vec();
        total += bytes.len();
        if bytes.len() > crate::document_parser::MAX_UPLOAD || total > 64 * 1024 * 1024 {
            return Err(Error::invalid(
                "Use files up to 16 MiB and batches up to 64 MiB.",
            ));
        }
        documents.push((name, bytes));
    }
    if documents.is_empty() {
        return Err(Error::invalid(
            "Select at least one requirement or Excel file.",
        ));
    }
    let documents = blocking(move || {
        documents
            .into_iter()
            .map(|(name, bytes)| {
                let content = crate::document_parser::parse(&name, &bytes)?;
                Ok(data::sources::Document {
                    name,
                    bytes,
                    content,
                })
            })
            .collect::<Result<Vec<_>>>()
    })
    .await?;
    crate::data_api::run(
        app,
        scope,
        true,
        |_, _| Ok(()),
        move |c, s| data::sources::import(c, s, documents),
    )
    .await
}
fn source_download(name: String, bytes: Vec<u8>, xlsx: bool) -> Response {
    let encoded = name
        .bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || b"-._".contains(&b) {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect::<String>();
    (
        [
            (
                "content-type",
                if xlsx {
                    "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet".into()
                } else {
                    "application/octet-stream".into()
                },
            ),
            (
                "content-disposition",
                format!("attachment; filename*=UTF-8''{encoded}"),
            ),
            ("x-content-type-options", "nosniff".into()),
        ],
        bytes,
    )
        .into_response()
}
async fn source_original(
    State(app): State<App>,
    Query(scope): Query<Scope>,
    Path(id): Path<String>,
) -> Result<Response> {
    let s = scope.id()?;
    let (name, bytes) =
        blocking(move || data::sources::original(&app.store.connect()?, &s, &id)).await?;
    Ok(source_download(name, bytes, false))
}
async fn workbook_download(
    State(app): State<App>,
    Query(scope): Query<Scope>,
    Path(id): Path<String>,
) -> Result<Response> {
    let s = scope.id()?;
    let (name, bytes) =
        blocking(move || data::sources::download(&app.store.connect()?, &s, &id)).await?;
    Ok(source_download(name, bytes, true))
}
async fn config(
    State(app): State<App>,
    Query(scope): Query<Scope>,
    headers: HeaderMap,
    Json(input): Json<Value>,
) -> Result<Json<Value>> {
    let mut blocked = Vec::new();
    for (name, prefix) in [("host", "http://"), ("origin", "")] {
        if let Some(value) = headers
            .get(name)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| reqwest::Url::parse(&format!("{prefix}{v}")).ok())
            .and_then(|u| u.port_or_known_default())
            && !blocked.contains(&value)
        {
            blocked.push(value);
        }
    }
    crate::data_api::run(
        app,
        scope,
        true,
        |_, _| Ok(()),
        move |c, s| {
            let mut saved = data::save_config(c, s, &input)?;
            if let Some(previous) = saved["blocked_ports"].as_array() {
                for n in previous {
                    if let Some(n) = n.as_u64() {
                        let n = n as u16;
                        if !blocked.contains(&n) {
                            blocked.push(n);
                        }
                    }
                }
            }
            for origin in saved["allowed_origins"].as_array().unwrap() {
                if reqwest::Url::parse(origin.as_str().unwrap())
                    .ok()
                    .and_then(|u| u.port_or_known_default())
                    .is_some_and(|port| blocked.contains(&port))
                {
                    return Err(Error::invalid(
                        "Choose a separate test website, not this application's UI or API service.",
                    ));
                }
            }
            saved["blocked_ports"] = json!(blocked);
            c.execute(
                "UPDATE native_browser_config SET payload_json=? WHERE workspace_id=?",
                params![saved.to_string(), s],
            )?;
            Ok(saved)
        },
    )
    .await
}
async fn create(
    State(app): State<App>,
    Query(scope): Query<Scope>,
    Json(input): Json<Value>,
) -> Result<Json<Value>> {
    let key = id("test");
    crate::data_api::run(
        app,
        scope,
        true,
        |_, _| Ok(()),
        move |c, s| data::save_case(c, s, &key, &input),
    )
    .await
}
async fn case(
    State(app): State<App>,
    Query(scope): Query<Scope>,
    Path(key): Path<String>,
) -> Result<Json<Value>> {
    crate::data_api::run(
        app,
        scope,
        false,
        |_, _| Ok(()),
        move |c, s| data::case(c, s, &key),
    )
    .await
}
async fn update(
    State(app): State<App>,
    Query(scope): Query<Scope>,
    Path(key): Path<String>,
    Json(input): Json<Value>,
) -> Result<Json<Value>> {
    crate::data_api::run(
        app,
        scope,
        true,
        |_, _| Ok(()),
        move |c, s| {
            data::case(c, s, &key)?;
            data::save_case(c, s, &key, &input)
        },
    )
    .await
}
async fn delete(
    State(app): State<App>,
    Query(scope): Query<Scope>,
    Path(key): Path<String>,
    Json(input): Json<Value>,
) -> Result<Json<Value>> {
    crate::data_api::run(
        app,
        scope,
        true,
        |_, _| Ok(()),
        move |c, s| {
            let current = data::case(c, s, &key)?;
            if current["revision"] != input["expected_revision"] {
                return Err(Error::conflict("Test revision changed."));
            }
            c.execute(
                "DELETE FROM native_browser_cases WHERE workspace_id=? AND id=?",
                params![s, key],
            )?;
            Ok(json!({"success":true}))
        },
    )
    .await
}
async fn export(
    State(app): State<App>,
    Query(scope): Query<Scope>,
    Path(key): Path<String>,
) -> Result<Response> {
    let scope = scope.id()?;
    let store = app.store.clone();
    let (case, config) = blocking(move || {
        let c = store.connect()?;
        Ok((
            data::case(&c, &scope, &key)?,
            data::parse_selection(data::config(&c, &scope)?)?,
        ))
    })
    .await?;
    let output=data::worker(json!({"v":1,"operation":"export","config":config,"steps":case["steps"],"title":case["title"]})).await?;
    Ok((
        [
            ("content-type", "text/javascript; charset=utf-8"),
            (
                "content-disposition",
                "attachment; filename=browser-test.spec.mjs",
            ),
        ],
        text(&output, "source").to_owned(),
    )
        .into_response())
}
fn job_key(scope: &str, id: &str) -> String {
    serde_json::to_string(&(scope, id)).unwrap()
}
async fn start(
    State(app): State<App>,
    Query(scope): Query<Scope>,
    Json(input): Json<Value>,
) -> Result<Json<Value>> {
    if *app.shutdown_receiver().borrow() {
        return Err(Error::conflict("Server is shutting down."));
    }
    let scope = scope.id()?;
    if input.as_object().is_none_or(|m| {
        m.keys()
            .any(|k| !["command_id", "case_id", "expected_revision", "url"].contains(&k.as_str()))
    }) {
        return Err(Error::invalid("Invalid browser run request."));
    }
    let command = crate::learning::string(&input, "command_id", 128, true)?;
    if !command
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        return Err(Error::invalid("Invalid browser run identity."));
    }
    let digest = format!("{:x}", Sha256::digest(input.to_string().as_bytes()));
    let store = app.store.clone();
    let saved_scope = scope.clone();
    let saved_id = command.clone();
    let reservation=blocking(move||{
        let mut conn=store.connect()?;let tx=conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let previous:Option<String>=tx.query_row("SELECT request_digest FROM native_browser_runs WHERE workspace_id=? AND id=?",params![saved_scope,saved_id],|r|r.get(0)).optional()?;
        if let Some(previous)=previous{
            if previous!=digest{return Err(Error::conflict("Run identity belongs to a different request."));}
            return Ok((data::report(&tx,&saved_scope,&saved_id)?,None));
        }
        let config=data::parse_selection(data::config(&tx,&saved_scope)?)?;
        let case=if let Some(key)=input["case_id"].as_str(){
            let case=data::case(&tx,&saved_scope,key)?;
            if input["expected_revision"].as_u64()!=case["revision"].as_u64(){return Err(Error::conflict("Test revision changed. Reload before running."));}case
        }else{json!({"title":"Page inspection","steps":[{"action":"navigate","url":input["url"].as_str().unwrap_or(&config.base_url)}]})};
        data::validate_urls(&case["steps"],&config)?;
        let row=json!({"id":saved_id,"case_id":case["id"],"case_revision":case["revision"],"status":"running","created_at":now(),"report":null,"artifacts":[]});
        tx.execute("INSERT INTO native_browser_runs(workspace_id,id,case_id,request_digest,status,payload_json,created_at) VALUES (?,?,?,?,'running',?,?)",params![saved_scope,saved_id,case["id"].as_str(),digest,row.to_string(),now()])?;
        tx.commit()?;Ok((row,Some((case,config))))
    }).await?;
    if let Some((case, config)) = reservation.1 {
        let key = job_key(&scope, &command);
        let (cancel, mut cancelled) = watch::channel(false);
        app.browser_jobs
            .running
            .lock()
            .await
            .insert(key.clone(), cancel);
        tokio::spawn(async move {
            let work = data::worker(
                json!({"v":1,"operation":if case["id"].is_string(){"run"}else{"inspect"},"config":config,"title":case["title"],"steps":case["steps"]}),
            );
            let output = tokio::select! {
                value=work=>match value{Ok(value)=>value,Err(error)=>json!({"report":{"status":"failed","error":error.message}})},
                _=cancelled.changed()=>json!({"report":{"status":"cancelled","error":"Run cancelled."}}),
            };
            let store = app.store.clone();
            let fallback_store = store.clone();
            let fallback_scope = scope.clone();
            let fallback_command = command.clone();
            let committed = blocking(move || {
                let mut c = store.connect()?;
                let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
                data::record(
                    &tx,
                    &scope,
                    &command,
                    case["id"].as_str(),
                    case["revision"].as_u64(),
                    output,
                )?;
                tx.commit()?;
                Ok(())
            })
            .await;
            if committed.is_err() {
                let _=blocking(move||{
                    let row=json!({"id":fallback_command,"status":"failed","report":{"status":"failed","error":"Unable to save browser report."},"artifacts":[]});
                    fallback_store.connect()?.execute("UPDATE native_browser_runs SET status='failed',payload_json=? WHERE workspace_id=? AND id=?",params![row.to_string(),fallback_scope,fallback_command])?;Ok(())
                }).await;
            }
            app.browser_jobs.running.lock().await.remove(&key);
        });
    }
    Ok(Json(reservation.0))
}
async fn report(
    State(app): State<App>,
    Query(scope): Query<Scope>,
    Path(key): Path<String>,
) -> Result<Json<Value>> {
    crate::data_api::run(
        app,
        scope,
        false,
        |_, _| Ok(()),
        move |c, s| data::report(c, s, &key),
    )
    .await
}
async fn cancel(
    State(app): State<App>,
    Query(scope): Query<Scope>,
    Path(key): Path<String>,
) -> Result<Json<Value>> {
    let scope = scope.id()?;
    data::report(&app.store.connect()?, &scope, &key)?;
    if let Some(sender) = app
        .browser_jobs
        .running
        .lock()
        .await
        .get(&job_key(&scope, &key))
    {
        let _ = sender.send(true);
    }
    Ok(Json(json!({"accepted":true})))
}
async fn artifact(
    State(app): State<App>,
    Query(scope): Query<Scope>,
    Path(key): Path<String>,
) -> Result<Response> {
    let scope = scope.id()?;
    let store = app.store.clone();
    let (name, mime, bytes): (String, String, Vec<u8>) = blocking(move || {
        store
            .connect()?
            .query_row(
                "SELECT name,mime,data FROM native_browser_artifacts WHERE workspace_id=? AND id=?",
                params![scope, key],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?
            .ok_or_else(Error::missing)
    })
    .await?;
    Ok((
        [
            ("content-type", mime),
            (
                "content-disposition",
                format!("attachment; filename=\"{name}\""),
            ),
            ("x-content-type-options", "nosniff".to_owned()),
        ],
        bytes,
    )
        .into_response())
}
