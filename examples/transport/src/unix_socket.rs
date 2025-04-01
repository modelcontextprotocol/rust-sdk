use std::fs;

use common::calculator::Calculator;
use rmcp::{serve_client, serve_server};
use tokio::net::{UnixListener, UnixStream};

mod common;

const SOCKET_PATH: &str = "/tmp/rmcp_example.sock";

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Remove any existing socket file
    let _ = fs::remove_file(SOCKET_PATH);
    match UnixListener::bind(SOCKET_PATH) {
        Ok(unix_listener) => {
            println!("Server successfully listening on {}", SOCKET_PATH);
            tokio::spawn(server(unix_listener));
        }
        Err(e) => {
            println!("Unable to bind to {}: {}", SOCKET_PATH, e);
        }
    }

    client().await?;

    // Clean up socket file
    let _ = fs::remove_file(SOCKET_PATH);

    Ok(())
}

async fn server(unix_listener: UnixListener) -> anyhow::Result<()> {
    while let Ok((stream, addr)) = unix_listener.accept().await {
        println!("Client connected: {:?}", addr);
        tokio::spawn(async move {
            match serve_server(Calculator, stream).await {
                Ok(server) => {
                    println!("Server initialized successfully");
                    if let Err(e) = server.waiting().await {
                        println!("Error while server waiting: {}", e);
                    }
                }
                Err(e) => println!("Server initialization failed: {}", e),
            }

            anyhow::Ok(())
        });
    }
    Ok(())
}

async fn client() -> anyhow::Result<()> {
    println!("Client connecting to {}", SOCKET_PATH);
    let stream = UnixStream::connect(SOCKET_PATH).await?;

    let client = serve_client((), stream).await?;
    println!("Client connected and initialized successfully");

    // List available tools
    let tools = client.peer().list_tools(Default::default()).await?;
    println!("Available tools: {:?}", tools);

    // Call the sum tool
    if let Some(sum_tool) = tools.tools.iter().find(|t| t.name.contains("sum")) {
        println!("Calling sum tool: {}", sum_tool.name);
        let result = client
            .peer()
            .call_tool(rmcp::model::CallToolRequestParam {
                name: sum_tool.name.clone(),
                arguments: Some(rmcp::object!({
                    "a": 10,
                    "b": 20
                })),
            })
            .await?;

        println!("Result: {:?}", result);
    }

    Ok(())
}
