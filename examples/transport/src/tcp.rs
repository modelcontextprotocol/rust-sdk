use common::calculator::Calculator;
use rmcp::{serve_client, serve_server};

mod common;
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tokio::spawn(server());
    client().await?;
    Ok(())
}

async fn server() -> anyhow::Result<()> {
    let tcp_listener = tokio::net::TcpListener::bind("127.0.0.1:8001").await?;
    while let Ok((stream, _)) = tcp_listener.accept().await {
        tokio::spawn(async move {
            let server = serve_server(Calculator, stream).await?;
            server.waiting().await?;
            anyhow::Ok(())
        });
    }
    Ok(())
}

async fn client() -> anyhow::Result<()> {
    let stream = tokio::net::TcpSocket::new_v4()?
        .connect("127.0.0.1:8001".parse()?)
        .await?;
    let client = serve_client((), stream).await?;
    let tools = client.peer().list_tools(Default::default()).await?;
    println!("{:?}", tools);
    Ok(())
}
