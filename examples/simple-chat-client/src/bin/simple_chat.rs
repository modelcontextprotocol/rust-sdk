use std::{process::exit, sync::Arc};

use anyhow::Result;
use clap::{Parser, Subcommand};
use simple_chat_client::{
    chat::ChatSession,
    client::OpenAIClient,
    config::Config,
    tool::{Tool, ToolSet, get_mcp_tools},
};

#[derive(Parser)]
#[command(author, version, about = "Simple Chat Client")]
struct Cli {
    /// Config file path
    #[arg(short, long, value_name = "FILE")]
    config: Option<String>,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Output default config template
    Config,

    /// Start chat
    Chat {
        /// Specify the model name
        #[arg(short, long)]
        model: Option<String>,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Config => {
            println!("{}", include_str!("../config.toml"));
            return Ok(());
        }
        Commands::Chat { model } => {
            // load config
            let config_path = cli.config;
            let mut config = match config_path {
                Some(path) => Config::load(&path).await?,
                None => {
                    println!("No config file provided, using default config");
                    exit(-1);
                }
            };

            // if command line specify model, override config file setting
            if let Some(model_name) = model {
                config.model_name = Some(model_name);
            }

            // create openai client
            let api_key = config
                .openai_key
                .clone()
                .unwrap_or_else(|| std::env::var("OPENAI_API_KEY").expect("need set api key"));
            let url = config.chat_url.clone();
            println!("use api address: {:?}", url);
            let openai_client = Arc::new(OpenAIClient::new(api_key, url, config.proxy));

            // create tool set
            let mut tool_set = ToolSet::default();

            // load MCP
            if config.mcp.is_some() {
                let mcp_clients = config.create_mcp_clients().await?;

                for (name, client) in mcp_clients {
                    println!("load MCP tool: {}", name);
                    let server = client.peer().clone();
                    let tools = get_mcp_tools(server).await?;

                    for tool in tools {
                        println!("add tool: {}", tool.name());
                        tool_set.add_tool(tool);
                    }
                }
            }

            // create chat session
            let mut session = ChatSession::new(
                openai_client,
                tool_set,
                config
                    .model_name
                    .unwrap_or_else(|| "gpt-4o-mini".to_string()),
            );

            let support_tool = config.support_tool.unwrap_or(true);
            let mut system_prompt;
            // if not support tool call, add tool call format guidance
            if !support_tool {
                // build system prompt
                system_prompt =
            "you are a assistant, you can help user to complete various tasks. you have the following tools to use:\n".to_string();

                // add tool info to system prompt
                for tool in session.get_tools() {
                    system_prompt.push_str(&format!(
                        "\ntool name: {}\ndescription: {}\nparameters: {}\n",
                        tool.name(),
                        tool.description(),
                        serde_json::to_string_pretty(&tool.parameters())
                            .expect("failed to serialize tool parameters")
                    ));
                }

                // add tool call format guidance
                system_prompt.push_str(
                    "\nif you need to call tool, please use the following format:\n\
            Tool: <tool name>\n\
            Inputs: <inputs>\n",
                );
                println!("system prompt: {}", system_prompt);
            } else {
                system_prompt =
                    "you are a assistant, you can help user to complete various tasks.".to_string();
            }

            // add system prompt
            session.add_system_prompt(system_prompt);

            // start chat
            session.chat(support_tool).await?;
        }
    }

    Ok(())
}
