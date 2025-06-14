use rig::{
    client::{CompletionClient, ProviderClient},
    embeddings::EmbeddingsBuilder,
    providers::{cohere, deepseek},
    vector_store::in_memory_store::InMemoryVectorStore,
};
use tracing_appender::rolling::{RollingFileAppender, Rotation};
pub mod chat;
pub mod config;
pub mod mcp_adaptor;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let file_appender = RollingFileAppender::new(
        Rotation::DAILY,
        "logs",
        format!("{}.log", env!("CARGO_CRATE_NAME")),
    );
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive(tracing::Level::INFO.into()),
        )
        .with_writer(file_appender)
        .with_file(false)
        .with_ansi(false)
        .init();

    let config = config::Config::retrieve("config.toml").await?;
    let openai_client = {
        if let Some(key) = config.deepseek_key {
            deepseek::Client::new(&key)
        } else {
            deepseek::Client::from_env()
        }
    };
    let cohere_client = {
        if let Some(key) = config.cohere_key {
            cohere::Client::new(&key)
        } else {
            cohere::Client::from_env()
        }
    };
    let mcp_manager = config.mcp.create_manager().await?;
    tracing::info!(
        "MCP Manager created, {} servers started",
        mcp_manager.clients.len()
    );
    let tool_set = mcp_manager.get_tool_set().await?;
    let embedding_model =
        cohere_client.embedding_model(cohere::EMBED_MULTILINGUAL_V3, "search_document");
    let embeddings = EmbeddingsBuilder::new(embedding_model.clone())
        .documents(tool_set.schemas()?)?
        .build()
        .await?;
    let store = InMemoryVectorStore::from_documents_with_id_f(embeddings, |f| {
        tracing::info!("store tool {}", f.name);
        f.name.clone()
    });
    let index = store.index(embedding_model);
    let dpsk = openai_client
        .agent(deepseek::DEEPSEEK_CHAT)
        .dynamic_tools(4, index, tool_set)
        .build();

    chat::cli_chatbot(dpsk).await?;

    Ok(())
}
