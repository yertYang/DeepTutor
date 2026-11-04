//! Explicit guardian grants, enforced independently for each learner operation.
use crate::{
    Application, Error, Result, accounts_api::Principal, application::blocking, config::write_json,
    store::text,
};
use axum::{
    Extension, Json, Router,
    extract::{Path, State},
    routing::{delete, get, post},
};
use serde_json::{Value, json};
use std::sync::Arc;
type App = Arc<Application>;
pub(crate) fn delegated_path(path: &str) -> bool {
    path == "/api/multi-user/me/guardianships"
        || path.starts_with("/api/multi-user/me/guardianships/")
        || path.starts_with("/api/multi-user/learners/")
}
pub(crate) fn router() -> Router<App> {
    Router::new()
        .route("/api/multi-user/guardians", get(list).post(authorize))
        .route("/api/multi-user/guardians/{id}", delete(revoke))
        .route("/api/multi-user/me/guardianships", get(mine))
        .route("/api/multi-user/me/guardianships/{id}", delete(revoke))
        .route("/api/multi-user/learners/{id}/guardian-report", get(report))
        .route(
            "/api/multi-user/learners/{id}/credentials/reset",
            post(reset),
        )
        .route(
            "/api/multi-user/learners/{id}/restrictions",
            get(restrictions).put(save_restrictions),
        )
        .route(
            "/api/multi-user/learners/{id}/materials",
            get(materials).put(save_materials),
        )
}
async fn list(State(app): State<App>) -> Result<Json<Value>> {
    Ok(Json(blocking(move || Ok(json!({"relationships":app.accounts.guardianships(None)?.into_iter().filter(|v|v["revoked_at"].is_null()).collect::<Vec<_>>()}))).await?))
}
async fn mine(State(app): State<App>, Extension(who): Extension<Principal>) -> Result<Json<Value>> {
    Ok(Json(blocking(move || Ok(json!({"relationships":app.accounts.guardianships(Some(text(&who.user,"id")))?.into_iter().filter(|v|v["revoked_at"].is_null()).collect::<Vec<_>>()}))).await?))
}
async fn authorize(State(app): State<App>, Json(body): Json<Value>) -> Result<Json<Value>> {
    Ok(Json(blocking(move || Ok(json!({"relationship":app.accounts.authorize_guardian(text(&body,"guardian_user_id"),text(&body,"learner_user_id"),&body["permissions"])?}))).await?))
}
async fn revoke(
    State(app): State<App>,
    Extension(who): Extension<Principal>,
    Path(id): Path<String>,
) -> Result<Json<Value>> {
    blocking(move || app.accounts.revoke_guardian(&who.user, &id)).await?;
    Ok(Json(json!({"ok":true})))
}
async fn reset(
    State(app): State<App>,
    Extension(who): Extension<Principal>,
    Path(id): Path<String>,
    Json(body): Json<Value>,
) -> Result<Json<Value>> {
    blocking(move || {
        app.accounts
            .reset_learner(&who.user, &id, text(&body, "new_password"))
    })
    .await?;
    Ok(Json(json!({"ok":true})))
}
async fn report(
    State(app): State<App>,
    Extension(who): Extension<Principal>,
    Path(id): Path<String>,
) -> Result<Json<Value>> {
    Ok(Json(blocking(move || {
        let user = app.accounts.guardian_permission(&who.user,&id,"view_reports")?;
        let grant = crate::accounts::grant(&app.settings,&id)?;
        let count = grant["models"]["llm"].as_array().into_iter().flatten().map(|p|p["model_ids"].as_array().map_or(0,Vec::len)).sum::<usize>();
        Ok(json!({"learner":{"id":id,"username":user["username"],"disabled":user["disabled"]},"assigned_materials":grant["learning_policy"]["reading"]["assigned_materials"].as_array().cloned().unwrap_or_default(),"grant_summary":{"model_count":count,"knowledge_base_count":grant["knowledge_bases"].as_array().map_or(0,Vec::len),"skill_count":grant["skills"].as_array().map_or(0,Vec::len)}}))
    }).await?))
}
fn current_policy(app: &Application, user: &Value) -> Result<Value> {
    crate::accounts_api::policy(&app.settings, user)
}
fn restriction_fields(policy: &Value) -> Value {
    json!({"age_band":policy["age_band"],"allow_upload":policy["reading"]["allow_upload"],"allowed_surfaces":policy["allowed_surfaces"],"extensions":policy["reading"]["extensions"]})
}
async fn restrictions(
    State(app): State<App>,
    Extension(who): Extension<Principal>,
    Path(id): Path<String>,
) -> Result<Json<Value>> {
    Ok(Json(blocking(move || {
        let user = app.accounts.guardian_permission(&who.user,&id,"manage_restrictions")?;
        Ok(json!({"restrictions":restriction_fields(&current_policy(&app,&user)?),"available_extensions":[]}))
    }).await?))
}
async fn save_restrictions(
    State(app): State<App>,
    Extension(who): Extension<Principal>,
    Path(id): Path<String>,
    Json(body): Json<Value>,
) -> Result<Json<Value>> {
    if !["6-8", "9-12", "13-15"].contains(&text(&body, "age_band"))
        || !body["allow_upload"].is_boolean()
        || body["allowed_surfaces"]
            .as_array()
            .is_none_or(|a| a.is_empty() || a.iter().any(|v| v != "chat" && v != "reading"))
        || body["extensions"].as_array().is_none_or(|a| !a.is_empty())
    {
        return Err(Error::invalid(
            "Invalid learner restrictions. Native extensions must be empty.",
        ));
    }
    Ok(Json(
        blocking(move || {
            let user = app
                .accounts
                .guardian_permission(&who.user, &id, "manage_restrictions")?;
            let _lock = app.settings.settings_lock()?;
            let mut grant = crate::accounts::grant(&app.settings, &id)?;
            let mut policy = current_policy(&app, &user)?;
            policy["age_band"] = body["age_band"].clone();
            policy["allowed_surfaces"] = body["allowed_surfaces"].clone();
            policy["allowed_capabilities"] = json!(
                body["allowed_surfaces"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|s| if s == "reading" {
                        "immersive_reading"
                    } else {
                        "chat"
                    })
                    .collect::<Vec<_>>()
            );
            policy["default_capability"] = json!(if body["allowed_surfaces"]
                .as_array()
                .unwrap()
                .iter()
                .any(|v| v == "chat")
            {
                "chat"
            } else {
                "immersive_reading"
            });
            policy["reading"]["allow_upload"] = body["allow_upload"].clone();
            policy["reading"]["extensions"] = json!([]);
            grant["learning_policy"] = policy.clone();
            write_json(&crate::accounts::grant_path(&app.settings, &id)?, &grant)?;
            Ok(json!({"restrictions":restriction_fields(&policy)}))
        })
        .await?,
    ))
}
fn available_materials(app: &Application) -> Result<Vec<Value>> {
    let c = app.store.connect()?;
    let mut offset = 0;
    let mut values = Vec::new();
    loop {
        let page = crate::reading::list(&c, "", offset)?;
        values.extend(page["items"].as_array().into_iter().flatten().cloned());
        let Some(next) = page["next_offset"].as_u64() else {
            break;
        };
        offset = next as u32;
    }
    Ok(values)
}
async fn materials(
    State(app): State<App>,
    Extension(who): Extension<Principal>,
    Path(id): Path<String>,
) -> Result<Json<Value>> {
    Ok(Json(blocking(move || {
        let user = app.accounts.guardian_permission(&who.user,&id,"assign_materials")?;
        let policy = current_policy(&app,&user)?;
        let assignments = policy["reading"]["assigned_materials"].as_array().cloned().unwrap_or_default();
        Ok(json!({"materials":available_materials(&app)?.iter().map(|item| json!({"book_id":item["id"],"title":item["title"],"assigned":assignments.iter().any(|v|v["book_id"] == item["id"]),"permission":"read"})).collect::<Vec<_>>()}))
    }).await?))
}
async fn save_materials(
    State(app): State<App>,
    Extension(who): Extension<Principal>,
    Path(id): Path<String>,
    Json(body): Json<Value>,
) -> Result<Json<Value>> {
    let ids = body["book_ids"]
        .as_array()
        .filter(|ids| ids.len() <= 100 && ids.iter().all(Value::is_string))
        .cloned()
        .ok_or_else(|| Error::invalid("Choose at most 100 reading materials."))?;
    let accounts = app.accounts.clone();
    let learner = id.clone();
    let user =
        blocking(move || accounts.guardian_permission(&who.user, &learner, "assign_materials"))
            .await?;
    let tenant = app.for_account(&user).await?;
    blocking(move || {
        let _lock = app.settings.settings_lock()?;
        let source = app.store.connect()?;
        let mut target = tenant.store.connect()?;
        let tx = target.transaction()?;
        let mut assignments = Vec::new();
        for id in ids {
            let copy = crate::reading::copy_material(&tx,&source,id.as_str().unwrap())?;
            if !assignments.iter().any(|v:&Value|v["item_id"] == copy["id"]) {
                assignments.push(json!({"book_id":id,"item_id":copy["id"],"title":copy["title"],"permission":"read"}));
            }
        }
        tx.commit()?;
        let mut grant = crate::accounts::grant(&app.settings,&id)?;
        let mut policy = current_policy(&app,&user)?;
        policy["reading"]["material_ids"] = json!(assignments.iter().map(|a|a["item_id"].clone()).collect::<Vec<_>>());
        policy["reading"]["assigned_materials"] = json!(assignments);
        grant["learning_policy"] = policy;
        write_json(&crate::accounts::grant_path(&app.settings,&id)?,&grant)
    }).await?;
    Ok(Json(json!({"ok":true})))
}
