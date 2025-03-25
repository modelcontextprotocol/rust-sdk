use std::{collections::HashMap, process::Stdio};

use rmcp::{RoleClient, ServiceExt, service::RunningService};
use serde::{Deserialize, Serialize};

use crate::mcp_adaptor::McpManager;
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct McpServerConfig {
    name: String,
    #[serde(flatten)]
    transport: McpServerTransportConfig,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(tag = "protocol", rename_all = "lowercase")]
pub enum McpServerTransportConfig {
    Sse {
        url: String,
    },
    Stdio {
        command: String,
        #[serde(default)]
        args: Vec<String>,
        #[serde(default)]
        envs: HashMap<String, String>,
    },
}

#[derive(Debug, Serialize, Deserialize)]
pub struct McpConfig {
    server: Vec<McpServerConfig>,
}

impl McpConfig {
    pub async fn create_manager(&self) -> anyhow::Result<McpManager> {
        let mut clients = HashMap::new();
        let mut task_set = tokio::task::JoinSet::<anyhow::Result<_>>::new();
        for server in &self.server {
            let server = server.clone();
            task_set.spawn(async move {
                let client = server.transport.start().await?;
                anyhow::Result::Ok((server.name.clone(), client))
            });
        }
        let start_up_result = task_set.join_all().await;
        for result in start_up_result {
            match result {
                Ok((name, client)) => {
                    clients.insert(name, client);
                }
                Err(e) => {
                    eprintln!("Failed to start server: {:?}", e);
                }
            }
        }
        Ok(McpManager { clients })
    }
}

impl McpServerTransportConfig {
    pub async fn start(&self) -> anyhow::Result<RunningService<RoleClient, ()>> {
        let client = match self {
            McpServerTransportConfig::Sse { url } => {
                let transport = rmcp::transport::SseTransport::start(url).await?;
                ().serve(transport).await?
            }
            McpServerTransportConfig::Stdio {
                command,
                args,
                envs,
            } => {
                let transport = rmcp::transport::TokioChildProcess::new(
                    tokio::process::Command::new(command)
                        .args(args)
                        .envs(envs)
                        .stderr(Stdio::null()),
                )?;
                ().serve(transport).await?
            }
        };
        Ok(client)
    }
}
