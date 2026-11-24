//! Run with the path to the built `servers_events_stdio` executable.

use futures::StreamExt;
use rmcp::{
    ClientLifecycleMode, ClientServiceExt,
    events::{
        client::{EventUpdate, EventsClient, SubscriptionOptions},
        wire::PollParams,
    },
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
    let bootstrap = events.poll(PollParams::new("demo.sample")).await?;
    let mut options = SubscriptionOptions::default();
    options.cursor = bootstrap.cursor;
    // The descriptor advertises both modes; subscribe prefers push.
    let mut subscription = events.subscribe("demo.sample", None, options).await?;
    while let Some(update) = subscription.next().await {
        match update {
            EventUpdate::Occurrence(event) => println!("event {}", event.event_id),
            EventUpdate::Checkpoint(cursor) => {
                println!("checkpoint {cursor}");
                break;
            }
            EventUpdate::Terminated(error) => return Err(error.into()),
            _ => {}
        }
    }
    drop(subscription);
    client.cancel().await?;
    Ok(())
}
