use clap::{Parser, Subcommand};
use deeptutor_server::{
    Application, api,
    capability::Capability,
    store::{terminal, text},
};
use serde_json::json;
use std::{net::IpAddr, path::PathBuf};

#[derive(Parser)]
#[command(
    name = env!("CARGO_BIN_NAME"),
    version = include_str!("../../../VERSION").trim(),
    about = "Lorevia native Rust runtime (no Python required)"
)]
struct Cli {
    /// Runtime root containing data/user; project-root .env is never read.
    #[arg(long, env = "DEEPTUTOR_HOME", default_value = ".", global = true)]
    home: PathBuf,
    #[command(subcommand)]
    command: Command,
}
#[derive(Subcommand)]
enum Command {
    #[command(hide = true)]
    InternalOcr {
        #[arg(long)]
        input: PathBuf,
        #[arg(long)]
        output: PathBuf,
        #[arg(long)]
        format: String,
    },
    /// Inspect, edit and restore native memory without a Python runtime.
    Memory {
        #[arg(long, default_value = "", global = true)]
        workspace: String,
        #[command(subcommand)]
        command: deeptutor_server::memory_cli::MemoryCommand,
    },
    /// Create a database snapshot, or an offline recovery package with --output.
    Backup {
        /// New directory outside home; includes settings and registered workspaces.
        #[arg(long)]
        output: Option<PathBuf>,
    },
    /// Verify a recovery package without opening or changing the runtime home.
    VerifyBackup {
        #[arg(long)]
        from: PathBuf,
    },
    /// Restore a verified recovery package into a NEW --home directory.
    Restore {
        #[arg(long)]
        from: PathBuf,
    },
    /// Answer a persisted ask_user question and continue its turn.
    Reply {
        #[arg(long, default_value = "")]
        workspace: String,
        #[arg(long)]
        turn: String,
        #[arg(long)]
        tool_call_id: String,
        #[arg(long)]
        command_id: Option<String>,
        text: String,
    },
    /// Authenticate with Codex and import the account model catalog.
    CodexLogin,
    /// Refresh the authenticated Codex model catalog.
    CodexModels,
    /// Serve the native HTTP and v2 WebSocket APIs.
    Serve {
        #[arg(long, default_value = "127.0.0.1")]
        host: IpAddr,
        #[arg(long, default_value_t = 8002)]
        port: u16,
    },
    /// Start the Rust backend and a built Next.js standalone frontend.
    Start {
        #[arg(long, default_value = "web/.next/standalone")]
        web: PathBuf,
        #[arg(long, default_value = "node")]
        node: PathBuf,
        #[arg(long, default_value_t = 8002)]
        backend_port: u16,
        #[arg(long, default_value_t = 3782)]
        port: u16,
        #[arg(long, default_value = "127.0.0.1")]
        frontend_host: IpAddr,
    },
    /// Send a chat turn through the same durable application service.
    Run {
        #[arg(long, default_value = "")]
        workspace: String,
        /// Attach an existing native knowledge base (repeat for multiple bases).
        #[arg(long)]
        kb: Vec<String>,
        /// Select an enabled optional tool from the native whitelist.
        #[arg(long)]
        tool: Vec<String>,
        content: String,
        /// Capability form, e.g. `yanzhizhou run deep_solve "problem"`.
        legacy_content: Option<String>,
        #[arg(long)]
        session: Option<String>,
        #[arg(long, requires = "model")]
        profile: Option<String>,
        #[arg(long, requires = "profile")]
        model: Option<String>,
        #[arg(long)]
        effort: Option<String>,
    },
}
#[tokio::main]
async fn main() {
    if let Err(error) = run().await {
        eprintln!("{error}");
        std::process::exit(1)
    }
}
async fn run() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();
    if let Command::InternalOcr {
        input,
        output,
        format,
    } = &cli.command
    {
        deeptutor_server::ocr::worker(input, output, format)?;
        return Ok(());
    }
    let recovery = match &cli.command {
        Command::Backup {
            output: Some(output),
        } => Some(deeptutor_server::recovery::create(&cli.home, output)?),
        Command::Backup { output: None } => {
            Some(deeptutor_server::recovery::database_snapshot(&cli.home)?)
        }
        Command::VerifyBackup { from } => Some(deeptutor_server::recovery::verify(from)?),
        Command::Restore { from } => Some(deeptutor_server::recovery::restore(from, &cli.home)?),
        _ => None,
    };
    if let Some(result) = recovery {
        println!("{}", serde_json::to_string_pretty(&result)?);
        return Ok(());
    }
    if let Command::Serve { host, .. } = &cli.command
        && !host.is_loopback()
    {
        return Err("Bind the native API to loopback and expose the Next.js frontend for multi-user access.".into());
    }
    let app = Application::open(&cli.home)?;
    if matches!(&cli.command, Command::Serve { .. } | Command::Start { .. }) {
        app.resume_pending().await?;
    }
    match cli.command {
        Command::InternalOcr { .. } => {
            unreachable!("OCR workers return before opening the application")
        }
        Command::Memory { workspace, command } => {
            println!(
                "{}",
                serde_json::to_string_pretty(&deeptutor_server::memory_cli::run(
                    &app, &workspace, command
                )?)?
            );
        }
        Command::Backup { .. } | Command::VerifyBackup { .. } | Command::Restore { .. } => {
            unreachable!()
        }
        Command::Reply {
            workspace,
            turn,
            tool_call_id,
            command_id,
            text,
        } => {
            app.reply_command(json!({"type":"submit_user_reply","protocol_version":"2.0","turn_id":turn,"tool_call_id":tool_call_id,"command_id":command_id.unwrap_or_else(||uuid::Uuid::new_v4().to_string()),"text":text}),&workspace).await?;
            stream_turn(app, turn, &workspace).await?;
        }
        Command::Start {
            web,
            node,
            backend_port,
            port,
            frontend_host,
        } => {
            deeptutor_server::launcher::start(app, web, node, backend_port, port, frontend_host)
                .await?;
        }
        Command::CodexLogin => {
            deeptutor_server::oauth::login(app.settings.clone()).await?;
            let result = deeptutor_server::oauth::refresh_models(app.settings.clone()).await?;
            println!("{result}");
        }
        Command::CodexModels => {
            let result = deeptutor_server::oauth::refresh_models(app.settings.clone()).await?;
            println!("{result}");
        }
        Command::Serve { host, port } => {
            let listener = tokio::net::TcpListener::bind((host, port)).await?;
            deeptutor_server::browser_testing::block_control_port(listener.local_addr()?.port());
            eprintln!(
                "Lorevia Rust listening on http://{} (protocol 2.0)",
                listener.local_addr()?
            );
            let shutdown = app.clone();
            axum::serve(listener, api::router(app.clone()))
                .with_graceful_shutdown(async move {
                    deeptutor_server::launcher::signal().await;
                    shutdown.shutdown().await;
                })
                .await?;
        }
        Command::Run {
            workspace,
            kb,
            tool,
            content,
            legacy_content,
            session,
            profile,
            model,
            effort,
        } => {
            let (capability, content) = if let Some(message) = legacy_content {
                (Capability::parse(&content)?, message)
            } else {
                (Capability::Chat, content)
            };
            let selection = match (profile, model) {
                (Some(profile), Some(model)) => {
                    json!({"profile_id":profile,"model_id":model,"reasoning_effort":effort})
                }
                _ if effort.is_some() => {
                    let saved = match &session {
                        Some(id) => {
                            app.store.session(id, &workspace)?["preferences"]["llm_selection"]
                                .clone()
                        }
                        None => serde_json::Value::Null,
                    };
                    let model = app.settings.model(&saved)?;
                    json!({"profile_id":model.profile_id,"model_id":model.model_id,"reasoning_effort":effort})
                }
                _ => serde_json::Value::Null,
            };
            let receipt=app.start(json!({"content":content,"capability":capability.id(),"session_id":session,"llm_selection":selection,"client_submission_id":uuid::Uuid::new_v4().to_string(),"workspace_id":workspace,"knowledge_bases":kb,"tools":if tool.is_empty(){serde_json::Value::Null}else{json!(tool)}})).await?;
            stream_turn(app, receipt.turn, &workspace).await?;
        }
    }
    Ok(())
}

async fn stream_turn(
    app: std::sync::Arc<Application>,
    turn: String,
    workspace: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut cursor = 0;
    loop {
        for event in app.events(&turn, cursor).await? {
            cursor = event["seq"].as_i64().unwrap_or(cursor);
            match text(&event, "type") {
                "content" => {
                    use std::io::Write;
                    print!("{}", text(&event, "content"));
                    std::io::stdout().flush()?;
                }
                "wait_for_input" => {
                    let current = app.pending_question(&turn, workspace).await?;
                    if current["tool_call_id"] != event["metadata"]["tool_call_id"] {
                        continue;
                    }
                    eprintln!(
                        "{}",
                        serde_json::to_string_pretty(&event["metadata"]["ask_user"])?
                    );
                    eprintln!(
                        "Turn is waiting. Reply with{}: yanzhizhou --home {} reply --workspace {} --turn {} --tool-call-id {} <answer>",
                        if cfg!(windows) { " PowerShell" } else { "" },
                        shell_argument(
                            &app.settings
                                .user_root
                                .parent()
                                .and_then(|p| p.parent())
                                .unwrap_or(std::path::Path::new("."))
                                .to_string_lossy()
                        ),
                        shell_argument(workspace),
                        shell_argument(&turn),
                        shell_argument(text(&event["metadata"], "tool_call_id"))
                    );
                    return Ok(());
                }
                "error" => eprintln!("{}", text(&event, "content")),
                "done" => {
                    println!();
                    if event["metadata"]["status"] != "completed" {
                        return Err("Turn did not complete.".into());
                    }
                    return Ok(());
                }
                _ => {}
            }
        }
        let state = app.operation("get_turn", json!({"turn_id":turn})).await?;
        if terminal(text(&state, "status")) && state["last_seq"].as_i64().unwrap_or(0) <= cursor {
            break;
        }
        tokio::select! {_=tokio::signal::ctrl_c()=>{app.shutdown().await;return Err("Cancelled.".into());},_=tokio::time::sleep(std::time::Duration::from_millis(50))=>{}}
    }
    Ok(())
}

fn shell_argument(value: &str) -> String {
    // Commands are displayed for PowerShell on Windows and a POSIX shell on
    // Unix. Provider call IDs and paths must never become shell syntax.
    let escaped = if cfg!(windows) {
        value.replace('\'', "''")
    } else {
        value.replace('\'', "'\\''")
    };
    format!("'{escaped}'")
}
