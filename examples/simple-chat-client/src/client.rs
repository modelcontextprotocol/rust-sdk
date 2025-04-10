use anyhow::Result;
use async_trait::async_trait;
use reqwest::Client as HttpClient;

use crate::model::{CompletionRequest, CompletionResponse};

#[async_trait]
pub trait ChatClient: Send + Sync {
    async fn complete(&self, request: CompletionRequest) -> Result<CompletionResponse>;
}

pub struct OpenAIClient {
    api_key: String,
    client: HttpClient,
    base_url: String,
}

impl OpenAIClient {
    pub fn new(api_key: String, url: Option<String>) -> Self {
        let base_url = url.unwrap_or("https://api.openai.com/v1/chat/completions".to_string());

        // create http client without proxy
        let client = HttpClient::builder()
            .no_proxy()
            .build()
            .unwrap_or_else(|_| HttpClient::new());

        Self {
            api_key,
            client,
            base_url,
        }
    }

    pub fn with_base_url(mut self, base_url: impl Into<String>) -> Self {
        self.base_url = base_url.into();
        self
    }
}

#[async_trait]
impl ChatClient for OpenAIClient {
    async fn complete(&self, request: CompletionRequest) -> Result<CompletionResponse> {
        println!("sending request to {}", self.base_url);
        println!("using api key: {}", self.api_key);
        let request_json = serde_json::to_string(&request)?;
        println!("request content: {}", request_json);
        // no proxy

        let response = self
            .client
            .post(&self.base_url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("Content-Type", "application/json")
            .json(&request)
            .send()
            .await?;

        if !response.status().is_success() {
            let error_text = response.text().await?;
            println!("API error: {}", error_text);
            return Err(anyhow::anyhow!("API Error: {}", error_text));
        }

        let completion: CompletionResponse = response.json().await?;
        Ok(completion)
    }
}
