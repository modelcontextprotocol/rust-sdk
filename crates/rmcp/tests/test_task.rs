use std::{any::Any, sync::Arc};

use rmcp::{
    ServerHandler,
    model::{ServerCapabilities, ServerInfo, ServerResult},
    task_manager::{AsyncHandler, OperationMessage, OperationProcessor, OperationResultTransport},
};
use tokio::sync::Mutex;

#[derive(Clone)]
pub struct Server {
    processor: Arc<Mutex<OperationProcessor>>,
}



impl ServerHandler for Server {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            capabilities: ServerCapabilities::builder()
                .enable_resources()
                .enable_resources_subscribe()
                .enable_resources_list_changed()
                .build(),
            ..Default::default()
        }
    }
    fn list_tasks(
        &self,
        _request: Option<rmcp::model::PaginatedRequestParam>,
        _context: rmcp::service::RequestContext<rmcp::RoleServer>,
    ) -> impl Future<Output = Result<rmcp::model::ListTasksResult, rmcp::ErrorData>> + Send + '_ {
        async move {
            let process_wrap = self.processor.lock().await;
            let running = process_wrap.list_running();
            Ok(rmcp::model::ListTasksResult {
                tasks: vec![],
                next_cursor: None,
                total: Some(running.len() as u64),
            })
        }
    }
    fn enqueue_task(
        &self,
        request: &rmcp::model::ClientRequest,
        context: rmcp::service::RequestContext<rmcp::RoleServer>,
    ) -> impl Future<Output = Result<Option<rmcp::model::ServerResult>, rmcp::ErrorData>> + Send + '_
    {
        let self_cloned = self.clone();
    let _requst_cloned = request.clone();
    let _ctx_cloned = context.clone();

        async move {
            let operation_id = context.id.to_string();
            let _req_cloned = _requst_cloned;
            let _ctx_cloned = _ctx_cloned;
            let _server_arc = self_cloned.clone();

            let handler_name = operation_id.clone();
            // Register a minimal handler that immediately returns a dummy transport
            let handler: AsyncHandler = Arc::new(move |op_msg: OperationMessage| {
                Box::pin(async move {
                    struct DummyTransport { id: String }
                    impl OperationResultTransport for DummyTransport {
                        fn operation_id(&self) -> &String { &self.id }
                        fn as_any(&self) -> &dyn Any { self }
                    }
                    Ok(Box::new(DummyTransport { id: op_msg.operation_id.clone() }) as Box<dyn OperationResultTransport>)
                })
            });

            self.processor
                .lock()
                .await
                .register_handler(handler_name.clone(), handler)
                .unwrap();
            self.processor
                .lock()
                .await
                .submit_operation(OperationMessage {
                    operation_id: operation_id.clone(),
                    metadata: {
                        let mut m = std::collections::HashMap::new();
                        m.insert("handler".to_string(), handler_name);
                        m
                    },
                    timeout_secs: None,
                })
                .unwrap();

            Ok(Some(ServerResult::empty(())))
        }
    }
}

#[tokio::test]
async fn test_task_handler() {
    let mut processor = OperationProcessor::new();

    // A minimal transport for asserting results
    struct DummyTransport { id: String }
    impl OperationResultTransport for DummyTransport {
        fn operation_id(&self) -> &String { &self.id }
        fn as_any(&self) -> &dyn Any { self }
    }

    // Register a trivial async handler that immediately returns success
    let handler_name = "h1".to_string();
    let handler: AsyncHandler = Arc::new(move |op: OperationMessage| {
        let id = op.operation_id.clone();
        Box::pin(async move {
            Ok(Box::new(DummyTransport { id }) as Box<dyn OperationResultTransport>)
        })
    });
    processor.register_handler(handler_name.clone(), handler).expect("register handler");

    // Submit an operation routed to the above handler
    processor
        .submit_operation(OperationMessage {
            operation_id: "op1".to_string(),
            metadata: {
                let mut m = std::collections::HashMap::new();
                m.insert("handler".to_string(), handler_name);
                m
            },
            timeout_secs: None,
        })
        .expect("submit operation");

    // Give the spawned task a brief moment to run
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;

    // Collect completed results and assert
    let completed = processor.collect_completed_results();
    assert_eq!(completed.len(), 1);
    let result = &completed[0];
    assert_eq!(result.task_id, "op1");
    assert!(result.result.is_ok());
}


#[tokio::test]
async fn test_server_enqueue_task() {
   
}
