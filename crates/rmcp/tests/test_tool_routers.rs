#![cfg(not(feature = "local"))]
use std::{
    collections::HashMap,
    sync::atomic::{AtomicUsize, Ordering},
};

use futures::future::BoxFuture;
use rmcp::{
    ServerHandler,
    handler::server::{router::tool::ToolRouter, tool::CallToolHandler, wrapper::Parameters},
};

#[derive(Debug, Default)]
pub struct TestHandler<T: 'static = ()> {
    pub _marker: std::marker::PhantomData<fn(*const T)>,
}

impl<T: 'static> ServerHandler for TestHandler<T> {}
#[derive(Debug, schemars::JsonSchema, serde::Deserialize, serde::Serialize)]
pub struct Request {
    pub fields: HashMap<String, String>,
}

#[derive(Debug, schemars::JsonSchema, serde::Deserialize, serde::Serialize)]
pub struct Sum {
    pub a: i32,
    pub b: i32,
}

#[rmcp::tool_router(router = test_router_1)]
impl<T> TestHandler<T> {
    #[rmcp::tool]
    async fn async_method(&self, Parameters(Request { fields }): Parameters<Request>) {
        drop(fields)
    }
}

#[rmcp::tool_router(router = test_router_2)]
impl<T> TestHandler<T> {
    #[rmcp::tool]
    fn sync_method(&self, Parameters(Request { fields }): Parameters<Request>) {
        drop(fields)
    }
}

#[rmcp::tool]
async fn async_function(Parameters(Request { fields }): Parameters<Request>) {
    drop(fields)
}

#[rmcp::tool]
fn async_function2<T>(_callee: &TestHandler<T>) -> BoxFuture<'_, ()> {
    Box::pin(async move {})
}

#[test]
fn test_tool_router() {
    let test_tool_router: ToolRouter<TestHandler<()>> = ToolRouter::<TestHandler<()>>::new()
        .with_route((async_function_tool_attr(), async_function))
        .with_route((async_function2_tool_attr(), async_function2))
        + TestHandler::<()>::test_router_1()
        + TestHandler::<()>::test_router_2();
    let tools = test_tool_router.list_all();
    assert_eq!(tools.len(), 4);
    assert_handler(TestHandler::<()>::async_method);
}

fn assert_handler<S, H, A>(_handler: H)
where
    H: CallToolHandler<S, A>,
{
}

#[test]
fn test_tool_router_list_all_is_sorted() {
    let router: ToolRouter<TestHandler<()>> = ToolRouter::<TestHandler<()>>::new()
        .with_route((async_function_tool_attr(), async_function))
        .with_route((async_function2_tool_attr(), async_function2))
        + TestHandler::<()>::test_router_1()
        + TestHandler::<()>::test_router_2();
    let tools = router.list_all();
    let names: Vec<&str> = tools.iter().map(|t| t.name.as_ref()).collect();
    let mut sorted = names.clone();
    sorted.sort();
    assert_eq!(
        names, sorted,
        "list_all() should return tools sorted alphabetically by name"
    );
}

fn build_router() -> ToolRouter<TestHandler<()>> {
    ToolRouter::<TestHandler<()>>::new()
        .with_route((async_function_tool_attr(), async_function))
        .with_route((async_function2_tool_attr(), async_function2))
        + TestHandler::<()>::test_router_1()
        + TestHandler::<()>::test_router_2()
}

#[test]
fn test_disable_route() {
    let mut router = build_router();
    assert_eq!(router.list_all().len(), 4);
    assert!(router.has_route("async_function"));
    assert!(router.get("async_function").is_some());

    assert!(router.disable_route("async_function"));

    assert_eq!(router.list_all().len(), 3);
    assert!(!router.has_route("async_function"));
    assert!(router.get("async_function").is_none());
    assert!(router.is_disabled("async_function"));

    // other tools unaffected
    assert!(router.has_route("async_function2"));
    assert!(router.get("async_function2").is_some());
    assert!(!router.is_disabled("async_function2"));
}

#[test]
fn test_enable_route() {
    let mut router = build_router();
    assert!(router.disable_route("async_function"));
    assert!(!router.has_route("async_function"));

    assert!(router.enable_route("async_function"));
    assert!(router.has_route("async_function"));
    assert!(router.get("async_function").is_some());
    assert!(!router.is_disabled("async_function"));
    assert_eq!(router.list_all().len(), 4);
}

#[test]
fn test_with_disabled_builder() {
    let router = build_router()
        .with_disabled("async_function")
        .with_disabled("sync_method");

    assert_eq!(router.list_all().len(), 2);
    assert!(!router.has_route("async_function"));
    assert!(!router.has_route("sync_method"));
    assert!(router.has_route("async_function2"));
    assert!(router.has_route("async_method"));
}

#[test]
fn test_disabled_tools_survive_merge() {
    let mut router_a = ToolRouter::<TestHandler<()>>::new()
        .with_route((async_function_tool_attr(), async_function));
    assert!(router_a.disable_route("async_function"));

    let router_b = ToolRouter::<TestHandler<()>>::new()
        .with_route((async_function2_tool_attr(), async_function2));

    router_a.merge(router_b);

    assert_eq!(router_a.list_all().len(), 1);
    assert!(router_a.is_disabled("async_function"));
    assert!(router_a.has_route("async_function2"));
}

#[test]
fn test_disable_nonexistent_tool() {
    let mut router = build_router();
    // should not panic; returns true because the name is newly added to disabled set
    assert!(router.disable_route("does_not_exist"));
    assert_eq!(router.list_all().len(), 4);
    // is_disabled returns false for tools not in the map
    assert!(!router.is_disabled("does_not_exist"));
}

#[test]
fn test_remove_route_preserves_disabled_state() {
    let mut router = build_router();
    assert!(router.disable_route("async_function"));
    assert!(router.is_disabled("async_function"));

    router.remove_route("async_function");
    assert!(!router.has_route("async_function"));
    // Disabled marker is preserved — is_disabled returns false (no route in map)
    // but re-adding will inherit the disabled state (tested separately)
    assert!(!router.is_disabled("async_function"));
}

#[test]
fn test_remove_route_then_readd_stays_disabled() {
    let mut router = build_router();
    assert!(router.disable_route("async_function"));

    router.remove_route("async_function");
    assert!(!router.has_route("async_function"));

    // Re-add the route — it should inherit the disabled state
    let other = ToolRouter::<TestHandler<()>>::new()
        .with_route((async_function_tool_attr(), async_function));
    router.merge(other);

    assert!(!router.has_route("async_function"));
    assert!(router.is_disabled("async_function"));
    assert!(router.get("async_function").is_none());
}

#[test]
fn test_into_iter_skips_disabled() {
    let router = build_router().with_disabled("async_function");
    let names: Vec<_> = router
        .into_iter()
        .map(|r| r.attr.name.to_string())
        .collect();
    assert_eq!(names.len(), 3);
    assert!(!names.contains(&"async_function".to_string()));
}

#[test]
fn test_pre_disable_before_add_route() {
    // Disabling a name before adding a route with that name should
    // result in the route being disabled once added.
    let router = ToolRouter::<TestHandler<()>>::new()
        .with_disabled("async_function")
        .with_route((async_function_tool_attr(), async_function));

    assert_eq!(router.list_all().len(), 0);
    assert!(router.is_disabled("async_function"));
    assert!(!router.has_route("async_function"));
}

#[test]
fn test_disabled_tool_invisible_across_all_queries() {
    let router = build_router().with_disabled("async_function");

    // Not listed
    let names: Vec<_> = router.list_all().iter().map(|t| t.name.clone()).collect();
    assert!(!names.contains(&"async_function".into()));
    // Not retrievable
    assert!(router.get("async_function").is_none());
    // Not routable
    assert!(!router.has_route("async_function"));
    // But known as disabled
    assert!(router.is_disabled("async_function"));
}

#[test]
fn test_disable_route_then_add_route_blocks_tool() {
    // Full pre-disable lifecycle via runtime mutation (not builder)
    let mut router = ToolRouter::<TestHandler<()>>::new();
    router.disable_route("async_function");

    // Add route after disabling — tool should be blocked
    let other = ToolRouter::<TestHandler<()>>::new()
        .with_route((async_function_tool_attr(), async_function));
    router.merge(other);

    assert!(router.is_disabled("async_function"));
    assert!(!router.has_route("async_function"));
    assert!(router.get("async_function").is_none());
    assert_eq!(router.list_all().len(), 0);
}

#[test]
fn test_disable_enable_return_false_cases() {
    let mut router = build_router();

    // Repeated disable returns false
    assert!(router.disable_route("async_function"));
    assert!(!router.disable_route("async_function"));

    // Enable returns true, then false on repeat
    assert!(router.enable_route("async_function"));
    assert!(!router.enable_route("async_function"));

    // Enable on name never disabled returns false
    assert!(!router.enable_route("async_function2"));

    // Enable on unknown name returns false
    assert!(!router.enable_route("unknown"));
}

// ── Notifier tests ──────────────────────────────────────────────────────

fn counter_notifier() -> (
    impl Fn() + Send + Sync + 'static,
    std::sync::Arc<AtomicUsize>,
) {
    let counter = std::sync::Arc::new(AtomicUsize::new(0));
    let c = counter.clone();
    let notifier = move || {
        c.fetch_add(1, Ordering::SeqCst);
    };
    (notifier, counter)
}

#[test]
fn test_notifier_fires_on_disable_and_enable() {
    let (notifier, counter) = counter_notifier();
    let mut router = build_router();
    router.set_notifier(notifier);

    assert!(router.disable_route("async_function"));
    assert_eq!(counter.load(Ordering::SeqCst), 1);

    assert!(!router.disable_route("async_function"));
    assert_eq!(counter.load(Ordering::SeqCst), 1);

    assert!(router.enable_route("async_function"));
    assert_eq!(counter.load(Ordering::SeqCst), 2);

    assert!(!router.enable_route("async_function"));
    assert_eq!(counter.load(Ordering::SeqCst), 2);
}

#[test]
fn test_notifier_skips_nonexistent_tools() {
    let (notifier, counter) = counter_notifier();
    let mut router = build_router();
    router.set_notifier(notifier);

    assert!(router.disable_route("does_not_exist"));
    assert_eq!(counter.load(Ordering::SeqCst), 0);

    assert!(router.enable_route("does_not_exist"));
    assert_eq!(counter.load(Ordering::SeqCst), 0);

    assert!(router.disable_route("future_tool"));
    assert_eq!(counter.load(Ordering::SeqCst), 0);
    assert!(router.enable_route("future_tool"));
    assert_eq!(counter.load(Ordering::SeqCst), 0);
}

#[test]
fn test_no_notifier_no_panic() {
    let mut router = build_router();
    assert!(router.disable_route("async_function"));
    assert!(router.enable_route("async_function"));
    assert!(router.disable_route("async_function"));
    assert!(!router.disable_route("async_function"));
}

#[test]
fn test_clone_shares_notifier() {
    let (notifier, counter) = counter_notifier();
    let mut router = build_router();
    router.set_notifier(notifier);
    let mut cloned = router.clone();

    assert!(cloned.disable_route("async_function"));
    assert_eq!(counter.load(Ordering::SeqCst), 1);

    assert!(router.disable_route("async_function"));
    assert_eq!(counter.load(Ordering::SeqCst), 2);

    cloned.clear_notifier();
    assert!(cloned.enable_route("async_function"));
    assert_eq!(counter.load(Ordering::SeqCst), 2);

    assert!(router.enable_route("async_function"));
    assert_eq!(counter.load(Ordering::SeqCst), 3);
}

#[test]
fn test_pre_init_disable_silent_but_correct() {
    let mut router = build_router();

    assert!(router.disable_route("async_function"));
    assert_eq!(router.list_all().len(), 3);
    assert!(!router.has_route("async_function"));

    let (notifier, counter) = counter_notifier();
    router.set_notifier(notifier);
    assert_eq!(counter.load(Ordering::SeqCst), 0);

    assert!(router.enable_route("async_function"));
    assert_eq!(counter.load(Ordering::SeqCst), 1);
}
