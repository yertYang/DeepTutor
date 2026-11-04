//! One bounded native provider request for auxiliary learning tasks.
use crate::{
    Application, Error, Result,
    application::blocking,
    capability::Capability,
    provider::{self, Delta},
};
use serde_json::{Value, json};
use std::{sync::Arc, time::Duration};
use tokio::sync::mpsc;

pub async fn text(
    app: Arc<Application>,
    instructions: &str,
    input: String,
) -> Result<(String, Value)> {
    let settings = app.settings.clone();
    let model = blocking(move || settings.model(&Value::Null)).await?;
    text_with_model(app.settings.clone(), model, instructions, input).await
}

pub(crate) async fn text_with_model(
    settings: Arc<crate::config::Settings>,
    model: crate::config::Model,
    instructions: &str,
    input: String,
) -> Result<(String, Value)> {
    let mut body = provider::request_body_scoped(
        &model,
        vec![json!({"role":"user","content":input})],
        "",
        instructions,
        Capability::Chat,
        None,
        &Default::default(),
    )?;
    if let Some(object) = body.as_object_mut() {
        object.remove("tools");
        object.remove("tool_choice");
    }
    crate::turn_engine::validate_body(&model, &body)?;
    let (tx, mut rx) = mpsc::channel(32);
    let future = provider::round(settings, &model, body, tx, false, None);
    tokio::pin!(future);
    let deadline = tokio::time::sleep(Duration::from_secs(90));
    tokio::pin!(deadline);
    let mut content = String::new();
    let mut usage = json!({});
    let mut bytes = 0usize;
    loop {
        tokio::select! {
            _=&mut deadline=>return Err(Error::provider("Learning task timed out.")),
            result=&mut future=>{
                let round=result?;
                if !round.calls.is_empty(){return Err(Error::provider("Learning task returned an unexpected tool call."));}
                while let Ok(delta)=rx.try_recv(){collect(delta,&mut content,&mut usage,&mut bytes)?;}
                if content.trim().is_empty(){return Err(Error::provider("Learning task returned an empty result."));}
                return Ok((content,usage));
            },
            Some(delta)=rx.recv()=>collect(delta,&mut content,&mut usage,&mut bytes)?,
        }
    }
}
fn collect(delta: Delta, content: &mut String, usage: &mut Value, bytes: &mut usize) -> Result<()> {
    match delta {
        Delta::Content(text) => {
            *bytes = bytes.saturating_add(text.len());
            content.push_str(&text);
        }
        Delta::Thinking(text) => *bytes = bytes.saturating_add(text.len()),
        Delta::Usage(value) => crate::turn_engine::accumulate_usage(usage, value),
        Delta::RequestStatus { .. } => {}
        _ => return Err(Error::provider("Unexpected learning task event.")),
    }
    if *bytes > 128 * 1024 || content.len() > 16384 {
        return Err(Error::provider("Learning task output exceeds its limit."));
    }
    Ok(())
}
