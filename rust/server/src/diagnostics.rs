//! Bounded, temporary diagnostics against the edited catalog. No settings are applied.
use crate::{Application, Error, Result, application::blocking, config, store::text};
use axum::{
    Json, Router,
    extract::{Path, State},
    response::{
        Sse,
        sse::{Event, KeepAlive},
    },
    routing::{get, post},
};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    convert::Infallible,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tokio::sync::watch;

#[derive(Default)]
pub(crate) struct Runtime(Mutex<HashMap<String, Run>>);
struct Run {
    service: String,
    created: Instant,
    events: watch::Sender<Vec<Value>>,
}
pub(crate) fn router() -> Router<Arc<Application>> {
    Router::new()
        .route("/api/settings/tests/{service}/start", post(start))
        .route("/api/settings/tests/{service}/{id}/events", get(events))
}
pub(crate) fn draft_catalog(app: &Application, body: &Value) -> Result<Value> {
    let _guard = app.settings.settings_lock()?;
    let current = app.settings.catalog_locked()?;
    let draft = crate::settings_api::read_draft(&app.settings)?;
    let reference = if draft["catalog"].is_object() {
        &draft["catalog"]
    } else {
        &current
    };
    config::restore_catalog(body["catalog"].clone(), reference)
}
async fn start(
    State(app): State<Arc<Application>>,
    Path(service): Path<String>,
    Json(body): Json<Value>,
) -> Result<Json<Value>> {
    if !["llm", "task", "embedding", "search"].contains(&service.as_str()) {
        return Err(Error::invalid(
            "Select a language, task, embedding or search service to diagnose.",
        ));
    }
    let source = app.clone();
    let catalog = blocking(move || draft_catalog(&source, &body)).await?;
    let id = uuid::Uuid::new_v4().to_string();
    let (sender, _) = watch::channel(vec![
        json!({"type":"started","message":"Testing the selected model."}),
    ]);
    {
        let mut runs = app
            .diagnostics
            .0
            .lock()
            .map_err(|_| Error::conflict("Diagnostics store unavailable."))?;
        runs.retain(|_, run| run.created.elapsed() < Duration::from_secs(600));
        if runs.len() >= 16 {
            return Err(Error::conflict(
                "Too many recent diagnostics. Retry after ten minutes.",
            ));
        }
        runs.insert(
            id.clone(),
            Run {
                service: service.clone(),
                created: Instant::now(),
                events: sender.clone(),
            },
        );
    }
    tokio::spawn(async move {
        let result =
            tokio::time::timeout(Duration::from_secs(95), diagnose(app, &service, catalog)).await;
        let result =
            result.unwrap_or_else(|_| Err(Error::provider("Model diagnostics timed out.")));
        sender.send_modify(|events| match result {
            Ok(mut output) => {
                events.append(&mut output);
                events.push(json!({"type":"completed","message":"Model responded successfully"}));
            }
            Err(error) => events.push(json!({"type":"failed","message":error.message})),
        });
    });
    Ok(Json(json!({"run_id":id})))
}
async fn events(
    State(app): State<Arc<Application>>,
    Path((service, id)): Path<(String, String)>,
) -> Result<Sse<impl futures_util::Stream<Item = std::result::Result<Event, Infallible>>>> {
    let receiver = {
        let runs = app
            .diagnostics
            .0
            .lock()
            .map_err(|_| Error::conflict("Diagnostics store unavailable."))?;
        let run = runs
            .get(&id)
            .filter(|run| {
                run.service == service && run.created.elapsed() < Duration::from_secs(600)
            })
            .ok_or_else(Error::missing)?;
        run.events.subscribe()
    };
    let stream = futures_util::stream::unfold(
        (receiver, 0usize, false),
        |(mut receiver, mut index, finished)| async move {
            if finished {
                return None;
            }
            loop {
                let value = receiver.borrow_and_update().get(index).cloned();
                if let Some(value) = value {
                    index += 1;
                    let finished = value["type"] == "completed" || value["type"] == "failed";
                    return Some((
                        Ok(Event::default().data(value.to_string())),
                        (receiver, index, finished),
                    ));
                }
                if receiver.changed().await.is_err() {
                    return None;
                }
            }
        },
    );
    Ok(Sse::new(stream).keep_alive(KeepAlive::new().interval(Duration::from_secs(10))))
}
async fn diagnose(app: Arc<Application>, service: &str, mut catalog: Value) -> Result<Vec<Value>> {
    if service == "search" {
        let id = text(&catalog["services"][service], "active_profile_id");
        let result = crate::search::preview(&catalog, id, "learning resources").await?;
        return Ok(vec![
            json!({"type":"response","snippet":format!("{} search results", result["sources"].as_array().map_or(0, Vec::len))}),
        ]);
    }
    if service == "embedding" {
        return embedding(&catalog).await;
    }
    if service == "task" {
        catalog["services"]["llm"] = catalog["services"]["task"].clone();
    }
    let model = config::resolve_model(&catalog, &Value::Null)?;
    let metadata = context_metadata(&model).await;
    let (response, _) = crate::generation::text_with_model(
        app.settings.clone(),
        model,
        "Reply briefly to this connection test. Do not use tools.",
        "Say hello.".into(),
    )
    .await?;
    let mut events =
        vec![json!({"type":"response","snippet":response.chars().take(1000).collect::<String>()})];
    events.push(match metadata {
        Some(capacity) => json!({"type":"context_window","context_window":capacity,"source":"metadata","detail":"Reported by the provider model inventory."}),
        None => json!({"type":"context_window","context_window":16384,"source":"default","detail":"Context capacity was not reported by this provider."}),
    });
    Ok(events)
}
async fn context_metadata(model: &config::Model) -> Option<u64> {
    if model.binding == "openai_codex" {
        return None;
    }
    let base = model.base_url.trim_end_matches('/');
    let base = ["/chat/completions", "/responses", "/messages"]
        .iter()
        .find_map(|suffix| base.strip_suffix(suffix))
        .unwrap_or(base);
    let mut request = config::http_client()
        .ok()?
        .get(format!("{base}/models"))
        .timeout(Duration::from_secs(5));
    if !model.api_key.is_empty() {
        request = request.bearer_auth(&model.api_key);
    }
    if model.format == "anthropic" {
        request = request
            .header("x-api-key", &model.api_key)
            .header("anthropic-version", "2023-06-01");
    }
    for (name, value) in model.extra_headers.as_object().into_iter().flatten() {
        request = request.header(name, value.as_str()?);
    }
    let response = request.send().await.ok()?;
    if !response.status().is_success() {
        return None;
    }
    let value = config::response_json(response, 2 * 1024 * 1024)
        .await
        .ok()?;
    let row = value["data"]
        .as_array()?
        .iter()
        .find(|row| row["id"] == model.model)?;
    [
        &row["context_window"],
        &row["context_length"],
        &row["max_model_len"],
    ]
    .into_iter()
    .find_map(|value| value.as_u64().or_else(|| value.as_str()?.parse().ok()))
    .filter(|n| *n > 0)
}

async fn embedding(catalog: &Value) -> Result<Vec<Value>> {
    let (profile, model) = crate::service_preview::selection(catalog, "embedding", &Value::Null)?;
    let connection = crate::service_preview::connection(catalog, profile, model, "embedding")?;
    let mut payload = json!({"model":model["model"],"input":"connection test"});
    if let Some(dimension) = model["dimension"]
        .as_u64()
        .or_else(|| text(model, "dimension").parse().ok())
    {
        if model["send_dimensions"] == true {
            payload["dimensions"] = json!(dimension);
        }
    }
    let response = crate::service_preview::request(&connection, "embeddings")?
        .json(&payload)
        .send()
        .await
        .map_err(|_| Error::provider("Embedding provider connection failed."))?;
    crate::service_preview::successful(&response)?;
    let value = config::response_json(response, 8 * 1024 * 1024).await?;
    let vector = value["data"][0]["embedding"]
        .as_array()
        .filter(|v| !v.is_empty() && v.len() <= 65536 && v.iter().all(Value::is_number))
        .ok_or_else(|| Error::provider("Provider returned an invalid embedding vector."))?;
    Ok(vec![
        json!({"type":"capabilities","detected_dim":vector.len(),"supported_dimensions":[vector.len()]}),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        body::{Body, to_bytes},
        http::{Request, StatusCode},
        response::IntoResponse,
    };
    use tower::ServiceExt;

    #[tokio::test]
    async fn diagnostics_probe_the_draft_model_and_stream_replayable_results() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}/v1", listener.local_addr().unwrap());
        let upstream = Router::new()
            .route("/v1/models",get(|| async { Json(json!({"data":[{"id":"draft-model","context_length":32768}]})) }))
            .route("/v1/chat/completions", post(|headers: axum::http::HeaderMap, Json(body): Json<Value>| async move {
                assert_eq!(headers["authorization"],"Bearer draft-secret");
                assert_eq!(body["model"],"draft-model");
                assert!(body["tools"].is_null());
                ([("content-type","text/event-stream")], "data: {\"choices\":[{\"delta\":{\"content\":\"hello\"},\"finish_reason\":null}]}\n\ndata: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n").into_response()
            }));
        let task = tokio::spawn(axum::serve(listener, upstream).into_future());
        let home = tempfile::tempdir().unwrap();
        let app = Application::open(home.path()).unwrap();
        let catalog = json!({"version":1,"connections":[],"services":{"llm":{"active_profile_id":"test","active_model_id":"m","profiles":[{"id":"test","binding":"custom","base_url":base,"api_key":"draft-secret","models":[{"id":"m","model":"draft-model"}]}]}}});
        let router = crate::api::router(app.clone());
        let response = router
            .clone()
            .oneshot(
                Request::post("/api/settings/tests/llm/start")
                    .header("host", "127.0.0.1")
                    .header("content-type", "application/json")
                    .body(Body::from(json!({"catalog":catalog}).to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body: Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 65536).await.unwrap()).unwrap();
        let path = format!("/api/settings/tests/llm/{}/events", text(&body, "run_id"));
        for _ in 0..2 {
            let response = router
                .clone()
                .oneshot(
                    Request::get(&path)
                        .header("host", "127.0.0.1")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            let bytes = tokio::time::timeout(
                Duration::from_secs(10),
                to_bytes(response.into_body(), 65536),
            )
            .await
            .unwrap()
            .unwrap();
            let events = String::from_utf8(bytes.to_vec()).unwrap();
            assert!(events.contains("hello"), "{events}");
            assert!(events.contains("32768"), "{events}");
            assert!(events.contains("completed"), "{events}");
            assert!(!events.contains("draft-secret"));
        }
        assert!(
            app.settings.catalog().unwrap()["services"]["llm"]["profiles"]
                .as_array()
                .unwrap()
                .is_empty()
        );
        task.abort();
    }
}
