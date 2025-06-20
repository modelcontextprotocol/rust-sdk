use common::calculator::Calculator;
use hyper::{
    Request, StatusCode,
    body::Incoming,
    header::{HeaderValue, UPGRADE},
};
use hyper_util::rt::TokioIo;
use rmcp::{RoleClient, ServiceExt, service::RunningService};
use tracing_subscriber::EnvFilter;
mod common;
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env().add_directive(tracing::Level::INFO.into()))
        .init();
    start_server().await?;
    let client = http_client("127.0.0.1:8001").await?;
    let tools = client.list_all_tools().await?;
    client.cancel().await?;
    tracing::info!("{:#?}", tools);
    Ok(())
}

async fn http_server(req: Request<Incoming>) -> Result<hyper::Response<String>, hyper::Error> {
    tokio::spawn(async move {
        let upgraded = hyper::upgrade::on(req).await?;
        let service = Calculator::new().serve(TokioIo::new(upgraded)).await?;
        service.waiting().await?;
        anyhow::Result::<()>::Ok(())
    });
    let mut response = hyper::Response::new(String::new());
    *response.status_mut() = StatusCode::SWITCHING_PROTOCOLS;
    response
        .headers_mut()
        .insert(UPGRADE, HeaderValue::from_static("mcp"));
    Ok(response)
}

async fn http_client(uri: &str) -> anyhow::Result<RunningService<RoleClient, ()>> {
    let tcp_stream = tokio::net::TcpStream::connect(uri).await?;
    let (mut s, c) =
        hyper::client::conn::http1::handshake::<_, String>(TokioIo::new(tcp_stream)).await?;
    tokio::spawn(c.with_upgrades());
    let mut req = Request::new(String::new());
    req.headers_mut()
        .insert(UPGRADE, HeaderValue::from_static("mcp"));
    let response = s.send_request(req).await?;
    let upgraded = hyper::upgrade::on(response).await?;
    let client = ().serve(TokioIo::new(upgraded)).await?;
    Ok(client)
}

async fn start_server() -> anyhow::Result<()> {
    let tcp_listener = tokio::net::TcpListener::bind("127.0.0.1:8001").await?;
    let service = hyper::service::service_fn(http_server);
    tokio::spawn(async move {
        while let Ok((stream, addr)) = tcp_listener.accept().await {
            tracing::info!("accepted connection from: {}", addr);
            let conn = hyper::server::conn::http1::Builder::new()
                .serve_connection(TokioIo::new(stream), service)
                .with_upgrades();
            tokio::spawn(conn);
        }
    });

    Ok(())
}
