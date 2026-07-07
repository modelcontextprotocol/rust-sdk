use std::{any::Any, time::Duration};

use rmcp::{
    model::TaskStatusNotificationParam,
    task_manager::{
        OperationDescriptor, OperationMessage, OperationProcessor, OperationResultTransport,
    },
};
use serde_json::json;

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

#[tokio::test]
async fn ttl_is_interpreted_as_milliseconds() {
    let mut processor = OperationProcessor::new();
    let descriptor = OperationDescriptor::new("slow", "dummy").with_ttl(50);
    let future = Box::pin(async {
        tokio::time::sleep(Duration::from_millis(500)).await;
        Ok(Box::new(DummyTransport {
            id: "slow".to_string(),
            value: 0,
        }) as Box<dyn OperationResultTransport>)
    });

    processor
        .submit_operation(OperationMessage::new(descriptor, future))
        .expect("submit operation");

    tokio::time::sleep(Duration::from_millis(200)).await;
    let results = processor.peek_completed();
    assert_eq!(
        results.len(),
        1,
        "50ms ttl should have timed out the operation well within 200ms"
    );
    match &results[0].result {
        Err(err) => assert!(
            err.to_string().contains("timed out"),
            "unexpected error: {err}"
        ),
        Ok(_) => panic!("expected the operation to time out, but it completed"),
    }
}

#[test]
fn task_status_notification_param_preserves_meta() {
    let raw = json!({
        "_meta": {
            "traceId": "trace-1"
        },
        "taskId": "task-1",
        "status": "working",
        "createdAt": "2026-06-24T00:00:00Z",
        "lastUpdatedAt": "2026-06-24T00:00:01Z",
        "ttl": null
    });

    let params: TaskStatusNotificationParam = serde_json::from_value(raw).unwrap();

    assert_eq!(params.task.task_id, "task-1");
    assert_eq!(params.task_id, "task-1");
    assert_eq!(params.meta.as_ref().unwrap().0["traceId"], json!("trace-1"));

    let serialized = serde_json::to_value(&params).unwrap();

    assert_eq!(serialized["_meta"]["traceId"], json!("trace-1"));
    assert_eq!(serialized["taskId"], json!("task-1"));
}
