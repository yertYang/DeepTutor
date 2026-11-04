//! Local CLI adapters for the same workspace-scoped memory transactions as HTTP.
use crate::{Application, Error, Result, learning};
use clap::Subcommand;
use rusqlite::TransactionBehavior;
use serde_json::{Value, json};
use std::path::PathBuf;

#[derive(Subcommand)]
pub enum MemoryCommand {
    /// List native L2/L3 documents, revisions and available operations.
    List,
    /// Read L2, L3, or one key such as preferences, profile or chat.
    Show {
        #[arg(default_value = "L3")]
        target: String,
    },
    /// Save a UTF-8 Markdown file, only if its expected revision still matches.
    Write {
        layer: String,
        key: String,
        #[arg(long)]
        file: PathBuf,
        #[arg(long)]
        expected_revision: u64,
    },
    /// List recoverable versions of one memory document.
    Versions { layer: String, key: String },
    /// Restore a version while retaining the current content in history.
    Restore {
        layer: String,
        key: String,
        version: i64,
        #[arg(long)]
        expected_revision: u64,
    },
}
pub fn run(app: &Application, scope: &str, command: MemoryCommand) -> Result<Value> {
    if scope.len() > 256 {
        return Err(Error::invalid("Invalid workspace scope."));
    }
    learning::bootstrap(&app.settings, &app.store, scope)?;
    let write = matches!(
        command,
        MemoryCommand::Write { .. } | MemoryCommand::Restore { .. }
    );
    let mut conn = app.store.connect()?;
    let tx = conn.transaction_with_behavior(if write {
        TransactionBehavior::Immediate
    } else {
        TransactionBehavior::Deferred
    })?;
    let result = match command {
        MemoryCommand::List => learning::memory_overview(&tx, scope)?,
        MemoryCommand::Show { target } => {
            let mut docs = Vec::new();
            for (layer, keys) in [
                ("L2", learning::SURFACES.as_slice()),
                ("L3", learning::SLOTS.as_slice()),
            ] {
                for key in keys {
                    if target == layer || target == *key {
                        docs.push(learning::read_memory(&tx, scope, layer, key)?);
                    }
                }
            }
            if docs.is_empty() {
                return Err(Error::invalid(
                    "Choose L2, L3, or a native memory document key.",
                ));
            }
            json!({"documents":docs})
        }
        MemoryCommand::Write {
            layer,
            key,
            file,
            expected_revision,
        } => {
            if std::fs::metadata(&file)?.len() > 512 * 1024 {
                return Err(Error::invalid("Memory file exceeds 512 KiB."));
            }
            let content = std::fs::read_to_string(file)?;
            learning::write_memory_checked(
                &tx,
                scope,
                &layer,
                &key,
                &json!({"content":content,"expected_revision":expected_revision}),
            )?
        }
        MemoryCommand::Versions { layer, key } => {
            learning::memory_versions(&tx, scope, &layer, &key)?
        }
        MemoryCommand::Restore {
            layer,
            key,
            version,
            expected_revision,
        } => learning::restore_memory(
            &tx,
            scope,
            &layer,
            &key,
            version,
            &json!({"expected_revision":expected_revision}),
        )?,
    };
    tx.commit()?;
    Ok(result)
}
