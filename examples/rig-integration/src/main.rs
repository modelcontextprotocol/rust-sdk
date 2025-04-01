use rig::{
    embeddings::EmbeddingsBuilder,
    providers::{
        cohere,
        deepseek::{self, DEEPSEEK_CHAT},
    },
    vector_store::in_memory_store::InMemoryVectorStore,
};
pub mod chat;
pub mod config;
pub mod mcp_adaptor;
#[tokio::main]
async fn main() -> anyhow::Result<()> {
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
    eprintln!(
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
        eprintln!("store tool {}", f.name);
        f.name.clone()
    });
    let index = store.index(embedding_model);
    let dpsk = openai_client
        .agent(DEEPSEEK_CHAT)
        .context(
r#"You are an assistant here to help the user to do some works. 
When you need to use tools, you should select which tool is most appropriate to meet the user's requirement.
Follow these instructions closely. 
1. Consider the user's request carefully and identify the core elements of the request.
2. Select which tool among those made available to you is appropriate given the context. 
3. This is very important: never perform the operation yourself and never give me the direct result. 
Always respond with the name of the tool that should be used and the appropriate inputs
in the following format:
Tool: <tool name>
Inputs: <list of inputs>"#,
        )
        .dynamic_tools(4, index, tool_set)
        .build();
    chat::cli_chatbot(dpsk).await?;
    Ok(())
}
