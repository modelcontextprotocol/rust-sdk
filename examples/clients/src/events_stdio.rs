//! Run with the path to the built `servers_events_stdio` executable.

use rmcp::{
    ClientLifecycleMode, ClientServiceExt,
    events::{client::EventsClient, wire::PollParams},
    model::{ClientConfig, ProtocolVersion},
    transport::TokioChildProcess,
};

#[tokio::main(flavor = "current_thread")]
async fn main() -> anyhow::Result<()> {
    let executable = std::env::args_os().nth(1).ok_or_else(|| {
        anyhow::anyhow!("usage: clients_events_stdio <servers_events_stdio executable>")
    })?;
    let transport = TokioChildProcess::new(tokio::process::Command::new(executable))?;
    let client = ClientConfig::default()
        .serve_with_lifecycle(
            transport,
            ClientLifecycleMode::Discover {
                preferred_versions: vec![ProtocolVersion::V_2026_07_28],
            },
        )
        .await?;
    let events = EventsClient::new(client.peer().clone());
    let page = events.list_page(None).await?;
    for event in page.events {
        println!("{}: {}", event.name, event.description);
    }
    let mut params = PollParams::new("demo.sample");
    let bootstrap = events.poll(params.clone()).await?;
    params.cursor = bootstrap.cursor;
    let result = events.poll(params).await?;
    println!(
        "{} occurrences; checkpoint {:?}",
        result.events.len(),
        result.cursor
    );
    client.cancel().await?;
    Ok(())
}
