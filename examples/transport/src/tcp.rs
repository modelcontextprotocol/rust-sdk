use common::calculator::Calculator;
use rmcp::{serve_client, serve_server};
use tracing::info;
use tracing_subscriber::EnvFilter;

mod common;
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env().add_directive("info".parse()?))
        .init();

    
    let (tx, rx) = tokio::sync::oneshot::channel();
    tokio::spawn(server(tx));
    rx.await??;
    client().await?;
    Ok(())
}

async fn server(ready_tx: tokio::sync::oneshot::Sender<anyhow::Result<()>>) {
    
    let bind_result = tokio::net::TcpListener::bind("127.0.0.1:8001").await;
    let listener = match bind_result {
        Ok(l) => l,
        Err(e) => {
            let _ = ready_tx.send(Err(e.into()));
            return;
        }
    };

    info!("Server listening on {}", listener.local_addr().unwrap());
    let _ = ready_tx.send(Ok(()));

    
    while let Ok((stream, addr)) = listener.accept().await {
        info!("Accepted connection from: {}", addr);
        
        tokio::spawn(async move {
            match serve_server(Calculator, stream).await {
                Ok(server) => {
                    if let Err(e) = server.waiting().await {
                        info!("Connection closed with error: {}", e);
                    }
                }
                Err(e) => {
                    info!("Failed to serve connection: {}", e);
                }
            };
        });
    }
}

async fn client() -> anyhow::Result<()> {
    info!("Client connecting to server...");
    let stream = tokio::net::TcpSocket::new_v4()?
        .connect("127.0.0.1:8001".parse()?)
        .await?;
    let client = serve_client((), stream).await?;
    info!("Client connected successfully");
    let tools = client.peer().list_tools(Default::default()).await?;
    println!("{:?}", tools);
    Ok(())
}
