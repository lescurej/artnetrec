use super::handler::DmxHandler;
use crate::session::SessionFacade;
use rmcp::transport::streamable_http_server::{
    session::local::LocalSessionManager, StreamableHttpServerConfig, StreamableHttpService,
};
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

pub struct McpServerHandle {
    _cancel: CancellationToken,
}

pub fn spawn(facade: SessionFacade, port: u16) -> Result<(u16, McpServerHandle), String> {
    let cancel = CancellationToken::new();
    let child = cancel.child_token();
    let facade_for_factory = facade.clone();
    let bound_port = std::sync::Arc::new(std::sync::Mutex::new(None::<u16>));
    let bound_port_setup = Arc::clone(&bound_port);
    let thread = std::thread::spawn(move || {
        let runtime = match tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
        {
            Ok(runtime) => runtime,
            Err(err) => {
                eprintln!("mcp: tokio runtime: {err}");
                return;
            }
        };
        runtime.block_on(async move {
            let addr = format!("127.0.0.1:{port}");
            let listener = match tokio::net::TcpListener::bind(&addr).await {
                Ok(listener) => listener,
                Err(err) => {
                    eprintln!("mcp: cannot bind {addr}: {err}");
                    return;
                }
            };
            let local = listener.local_addr().ok();
            if let Some(addr) = local {
                if let Ok(mut guard) = bound_port_setup.lock() {
                    *guard = Some(addr.port());
                }
            }
            let listen_port = local.map(|a| a.port()).unwrap_or(port);
            let config = StreamableHttpServerConfig::default()
                .with_legacy_session_mode(false)
                .with_json_response(true)
                .with_cancellation_token(child.clone());
            let facade_factory = facade_for_factory.clone();
            let service = StreamableHttpService::new(
                move || Ok(DmxHandler::new(facade_factory.clone(), listen_port)),
                Arc::new(LocalSessionManager::default()),
                config,
            );
            let router = axum::Router::new().nest_service("/mcp", service);
            let _ = axum::serve(listener, router)
                .with_graceful_shutdown(async move { child.cancelled_owned().await })
                .await;
        });
    });
    std::thread::sleep(std::time::Duration::from_millis(120));
    let actual = bound_port.lock().ok().and_then(|g| *g).unwrap_or(port);
    if thread.is_finished() {
        return Err(format!("MCP server stopped on port {actual}"));
    }
    Ok((actual, McpServerHandle { _cancel: cancel }))
}
