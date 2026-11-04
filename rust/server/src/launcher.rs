//! Own the frontend and native backend lifecycle without a Python launcher.
use crate::{Application, Error, Result, api};
use std::{future::IntoFuture, net::IpAddr, path::PathBuf, sync::Arc, time::Duration};

pub async fn signal() {
    #[cfg(unix)]
    {
        if let Ok(mut term) =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        {
            tokio::select! {_=tokio::signal::ctrl_c()=>{},_=term.recv()=>{}};
            return;
        }
    }
    let _ = tokio::signal::ctrl_c().await;
}
pub async fn start(
    app: Arc<Application>,
    web: PathBuf,
    node: PathBuf,
    backend_port: u16,
    frontend_port: u16,
    frontend_host: IpAddr,
) -> Result<()> {
    if frontend_port == backend_port && frontend_port != 0 {
        return Err(Error::invalid("Frontend and backend ports must differ."));
    }
    let web = web.canonicalize()?;
    if !web.join("server.js").is_file() {
        return Err(Error::invalid(
            "Use the Next.js standalone directory containing server.js.",
        ));
    }
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", backend_port)).await?;
    let address = listener.local_addr()?;
    crate::browser_testing::block_control_port(address.port());
    crate::browser_testing::block_control_port(frontend_port);
    let backend = format!("http://{address}");
    let mut frontend = tokio::process::Command::new(node)
        .arg(web.join("server.js"))
        .current_dir(&web)
        .env("HOSTNAME", frontend_host.to_string())
        .env("PORT", frontend_port.to_string())
        .env("NODE_ENV", "production")
        .env("DEEPTUTOR_API_BASE_URL", &backend)
        .env("NEXT_TELEMETRY_DISABLED", "1")
        .env_remove("ELECTRON_RUN_AS_NODE")
        .kill_on_drop(true)
        .spawn()?;
    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();
    let server = axum::serve(listener, api::router(app.clone()))
        .with_graceful_shutdown(async {
            let _ = shutdown_rx.await;
        })
        .into_future();
    tokio::pin!(server);
    eprintln!(
        "Lorevia native backend: {backend}; frontend: http://{frontend_host}:{frontend_port}"
    );
    let mut server_finished = false;
    let outcome = tokio::select! {
        _=signal()=>Ok(()),
        result=&mut server=>{server_finished=true;result.map_err(Error::from)},
        _=frontend.wait()=>Err(Error::conflict("Frontend process exited; native services have stopped."))
    };
    app.shutdown().await;
    let _ = shutdown_tx.send(());
    let _ = frontend.start_kill();
    let _ = frontend.wait().await;
    if !server_finished {
        let _ = tokio::time::timeout(Duration::from_secs(5), &mut server).await;
    }
    outcome
}
