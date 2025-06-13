use napi_derive::napi;
use rmcp::transport::SseTransport;
use rmcp::transport::sse::ReqwestSseClient;
use reqwest;

#[napi]
pub struct JsSseTransport {
    #[napi(skip)]
    pub inner: Option<SseTransport<ReqwestSseClient, reqwest::Error>>,
}

#[napi]
impl JsSseTransport {
    /// Async static constructor, just like PySseTransport::start
    #[napi(factory)]
    pub async fn start(url: String) -> napi::Result<Self> {
        println!("JsSseTransport.start received URL: {}", url);
        match SseTransport::start(&url).await {
            Ok(transport) => {
                println!("JsSseTransport.start successful");
                Ok(JsSseTransport { inner: Some(transport) })
            },
            Err(e) => {
                println!("JsSseTransport.start error: {}", e);
                Err(napi::Error::from_reason(e.to_string()))
            },
        }
    }
    // Add more methods for interacting with the transport as needed
}
