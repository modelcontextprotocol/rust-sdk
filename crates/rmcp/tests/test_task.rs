use std::{any::Any, time::Duration};

use rmcp::task_manager::{
    OperationDescriptor, OperationMessage, OperationProcessor, OperationResultTransport,
};

struct DummyTransport {
    id: String,
    value: u32,
}

impl OperationResultTransport for DummyTransport {
    fn operation_id(&self) -> &String {
        &self.id
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[tokio::test]
async fn executes_enqueued_future() {
    let mut processor = OperationProcessor::new();
    let descriptor = OperationDescriptor::new("op1", "dummy");
    let future = Box::pin(async {
        tokio::time::sleep(Duration::from_millis(10)).await;
        Ok(Box::new(DummyTransport {
            id: "op1".to_string(),
            value: 42,
        }) as Box<dyn OperationResultTransport>)
    });

    processor
        .submit_operation(OperationMessage::new(descriptor, future))
        .expect("submit operation");

    tokio::time::sleep(Duration::from_millis(30)).await;
    let results = processor.peek_completed();
    assert_eq!(results.len(), 1);
    let payload = results[0]
        .result
        .as_ref()
        .unwrap()
        .as_any()
        .downcast_ref::<DummyTransport>()
        .unwrap();
    assert_eq!(payload.value, 42);
}

#[tokio::test]
async fn rejects_duplicate_operation_ids() {
    let mut processor = OperationProcessor::new();
    let descriptor = OperationDescriptor::new("dup", "dummy");
    let future = Box::pin(async {
        Ok(Box::new(DummyTransport {
            id: "dup".to_string(),
            value: 1,
        }) as Box<dyn OperationResultTransport>)
    });
    processor
        .submit_operation(OperationMessage::new(descriptor, future))
        .expect("first submit");

    let descriptor_dup = OperationDescriptor::new("dup", "dummy");
    let future_dup = Box::pin(async {
        Ok(Box::new(DummyTransport {
            id: "dup".to_string(),
            value: 2,
        }) as Box<dyn OperationResultTransport>)
    });

    let err = processor
        .submit_operation(OperationMessage::new(descriptor_dup, future_dup))
        .expect_err("duplicate should fail");
    assert!(format!("{err}").contains("already running"));
}
