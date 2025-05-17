mod common;

#[cfg(target_family = "windows")]
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    use common::calculator::Calculator;
    use rmcp::{serve_client, serve_server};
    use tokio::net::windows::named_pipe::{ClientOptions, ServerOptions};
    const PIPE_NAME: &str = r"\\.\pipe\rmcp_example";

    async fn server(name: &str) -> anyhow::Result<()> {
        let mut server = ServerOptions::new()
            .first_pipe_instance(true)
            .create(name)?;
        while let Ok(_) = server.connect().await {
            let stream = server;
            server = ServerOptions::new().create(name)?;
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
        println!("Client connecting to {}", PIPE_NAME);
        let stream = ClientOptions::new().open(PIPE_NAME)?;

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
    tokio::spawn(server(PIPE_NAME));
    let mut clients = vec![];

    for _ in 0..100 {
        clients.push(client());
    }
    for client in clients {
        client.await?;
    }
    Ok(())
}

#[cfg(not(target_family = "windows"))]
fn main() {
    println!("Unix socket example is not supported on this platform.");
}
