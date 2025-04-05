use std::sync::Arc;

use rmcp::{
    ServerHandler,
    model::{ServerCapabilities, ServerInfo},
    schemars, tool,
};

#[allow(dead_code)]
pub trait DataService: Send + Sync + 'static {
    fn get_data(&self) -> String;
    fn set_data(&mut self, data: String);
}

#[derive(Debug, Clone)]
pub struct MemoryDataService {
    data: String,
}

impl MemoryDataService {
    #[allow(dead_code)]
    pub fn new(initial_data: impl Into<String>) -> Self {
        Self {
            data: initial_data.into(),
        }
    }
}

impl DataService for MemoryDataService {
    fn get_data(&self) -> String {
        self.data.clone()
    }

    fn set_data(&mut self, data: String) {
        self.data = data;
    }
}

#[derive(Debug, Clone)]
pub struct GenericService<DS: DataService> {
    #[allow(dead_code)]
    data_service: Arc<DS>,
}

#[tool(tool_box)]
impl<DS: DataService> GenericService<DS> {
    pub fn new(data_service: DS) -> Self {
        Self {
            data_service: Arc::new(data_service),
        }
    }

    #[tool(description = "get memory from service")]
    pub async fn get_data(&self) -> String {
        self.data_service.get_data()
    }

    #[tool(description = "set memory to service")]
    pub async fn set_data(&self, #[tool(param)] data: String) -> String {
        let new_data = data.clone();
        format!("Current memory: {}", new_data)
    }
}

impl<DS: DataService> ServerHandler for GenericService<DS> {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            instructions: Some("generic data service".into()),
            capabilities: ServerCapabilities::builder().enable_tools().build(),
            ..Default::default()
        }
    }
}
