use std::sync::Arc;

use rmcp::{
    ServerHandler, handler::server::wrapper::Parameters, schemars, tool, tool_handler, tool_router,
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

#[derive(Debug, schemars::JsonSchema, serde::Deserialize, serde::Serialize)]
pub struct SetDataRequest {
    pub data: String,
}

#[tool_router]
impl<DS: DataService> GenericService<DS> {
    #[allow(dead_code)]
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
    pub async fn set_data(
        &self,
        Parameters(SetDataRequest { data }): Parameters<SetDataRequest>,
    ) -> String {
        let new_data = data.clone();
        format!("Current memory: {}", new_data)
    }
}

#[tool_handler(instructions = "generic data service")]
impl<DS: DataService> ServerHandler for GenericService<DS> {}
