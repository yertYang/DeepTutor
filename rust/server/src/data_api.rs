//! Shared workspace validation and SQLite transaction boundary for data APIs.
use crate::{Application, Error, Result, application::blocking};
use axum::Json;
use rusqlite::{Connection, TransactionBehavior};
use serde::Deserialize;
use serde_json::Value;
use std::sync::Arc;

#[derive(Clone, Deserialize, Default)]
pub(crate) struct Scope {
    workspace_id: Option<String>,
    dt_workspace: Option<String>,
    pub(crate) notebook_id: Option<String>,
}

impl Scope {
    pub(crate) fn id(&self) -> Result<String> {
        if let (Some(a), Some(b)) = (&self.workspace_id, &self.dt_workspace)
            && a != b
        {
            return Err(Error::invalid("Conflicting workspace scopes."));
        }
        let id = self
            .dt_workspace
            .as_ref()
            .or(self.workspace_id.as_ref())
            .cloned()
            .unwrap_or_default();
        if id.len() > 256 {
            return Err(Error::invalid("Invalid workspace scope."));
        }
        Ok(id)
    }
}

pub(crate) async fn run(
    app: Arc<Application>,
    scope: Scope,
    write: bool,
    prepare: impl FnOnce(&Application, &str) -> Result<()> + Send + 'static,
    f: impl FnOnce(&Connection, &str) -> Result<Value> + Send + 'static,
) -> Result<Json<Value>> {
    let scope = scope.id()?;
    Ok(Json(
        blocking(move || {
            prepare(&app, &scope)?;
            let mut conn = app.store.connect()?;
            let tx = conn.transaction_with_behavior(if write {
                TransactionBehavior::Immediate
            } else {
                TransactionBehavior::Deferred
            })?;
            let value = f(&tx, &scope)?;
            tx.commit()?;
            Ok(value)
        })
        .await?,
    ))
}
