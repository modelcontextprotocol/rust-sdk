use rmcp::{
    ClientLifecycleMode, ClientServiceExt,
    model::{ClientInfo, ProtocolVersion, SubscriptionFilter},
    transport::StreamableHttpClientTransport,
};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    let transport = StreamableHttpClientTransport::from_uri("http://127.0.0.1:8000/mcp");
    let client = ClientInfo::default()
        .serve_with_lifecycle(
            transport,
            ClientLifecycleMode::Discover {
                preferred_versions: vec![ProtocolVersion::V_2026_07_28],
            },
        )
        .await?;
    let mut subscription = client
        .listen(SubscriptionFilter::builder().tools_list_changed().build())
        .await?;

    println!("accepted filter: {:?}", subscription.acknowledged());
    loop {
        tokio::select! {
            result = subscription.next() => {
                match result? {
                    Some(notification) => println!("notification: {notification:?}"),
                    None => {
                        println!("subscription ended: {:?}", subscription.end());
                        break;
                    }
                }
            }
            _ = tokio::signal::ctrl_c() => {
                subscription.cancel().await?;
                break;
            }
        }
    }

    client.cancel().await?;
    Ok(())
}
