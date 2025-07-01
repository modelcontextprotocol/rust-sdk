mod common;
use std::sync::Arc;

use actix_web::{App, HttpServer, middleware};
use common::counter::Counter;
use rmcp::transport::streamable_http_server::{
    StreamableHttpService, session::local::LocalSessionManager,
};

#[actix_web::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::DEBUG)
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let bind_addr = "127.0.0.1:8080";
    
    // Create the streamable HTTP service
    let service = Arc::new(StreamableHttpService::new(
        || Ok(Counter::new()),
        LocalSessionManager::default().into(),
        Default::default(),
    ));

    println!("Starting actix-web streamable HTTP server on {}", bind_addr);
    println!("POST / - Send JSON-RPC requests");
    println!("GET / - Resume SSE stream with session ID");
    println!("DELETE / - Close session");

    HttpServer::new(move || {
        App::new()
            .wrap(middleware::Logger::default())
            .configure(StreamableHttpService::configure(service.clone()))
    })
    .bind(bind_addr)?
    .run()
    .await?;

    Ok(())
}