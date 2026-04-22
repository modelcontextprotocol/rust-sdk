//! Integration tests for tool list change notifications.
#![cfg(all(feature = "client", not(feature = "local")))]

use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use rmcp::{
    ClientHandler, RoleClient, RoleServer, ServerHandler, ServiceExt,
    handler::server::{router::tool::ToolRoute, tool::ToolCallContext},
    model::{CallToolResult, ServerCapabilities, ServerInfo, Tool},
    service::{MaybeSendFuture, NotificationContext},
};
use tokio::sync::{Notify, RwLock};

#[derive(Clone)]
struct TestToolServer {
    router: Arc<RwLock<rmcp::handler::server::router::tool::ToolRouter<Self>>>,
    trigger_disable: Arc<Notify>,
    trigger_enable: Arc<Notify>,
}

impl TestToolServer {
    fn new() -> Self {
        let mut tool_router = rmcp::handler::server::router::tool::ToolRouter::<Self>::new();
        tool_router.add_route(ToolRoute::new_dyn(
            Tool::new("tool_a", "Tool A", Arc::new(Default::default())),
            |_ctx| Box::pin(async { Ok(CallToolResult::default()) }),
        ));
        tool_router.add_route(ToolRoute::new_dyn(
            Tool::new("tool_b", "Tool B", Arc::new(Default::default())),
            |_ctx| Box::pin(async { Ok(CallToolResult::default()) }),
        ));
        Self {
            router: Arc::new(RwLock::new(tool_router)),
            trigger_disable: Arc::new(Notify::new()),
            trigger_enable: Arc::new(Notify::new()),
        }
    }
}

impl ServerHandler for TestToolServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
    }

    fn call_tool(
        &self,
        request: rmcp::model::CallToolRequestParams,
        context: rmcp::service::RequestContext<RoleServer>,
    ) -> impl std::future::Future<Output = Result<CallToolResult, rmcp::ErrorData>> + MaybeSendFuture + '_
    {
        async move {
            let router = self.router.read().await;
            let tcc = ToolCallContext::new(self, request, context);
            router.call(tcc).await
        }
    }

    fn list_tools(
        &self,
        _request: Option<rmcp::model::PaginatedRequestParams>,
        _context: rmcp::service::RequestContext<RoleServer>,
    ) -> impl std::future::Future<Output = Result<rmcp::model::ListToolsResult, rmcp::ErrorData>>
    + MaybeSendFuture
    + '_ {
        async move {
            let router = self.router.read().await;
            Ok(rmcp::model::ListToolsResult {
                tools: router.list_all(),
                ..Default::default()
            })
        }
    }

    fn on_initialized(
        &self,
        context: NotificationContext<RoleServer>,
    ) -> impl std::future::Future<Output = ()> + MaybeSendFuture + '_ {
        let router = self.router.clone();
        let trigger_disable = self.trigger_disable.clone();
        let trigger_enable = self.trigger_enable.clone();
        let peer = context.peer.clone();

        async move {
            router.write().await.bind_peer_notifier(&peer);

            let router = router.clone();
            tokio::spawn(async move {
                trigger_disable.notified().await;
                {
                    let mut r = router.write().await;
                    r.disable_route("tool_a");
                }

                trigger_enable.notified().await;
                {
                    let mut r = router.write().await;
                    r.enable_route("tool_a");
                }
            });
        }
    }
}

#[derive(Clone)]
struct TestToolClient {
    notification_count: Arc<AtomicUsize>,
    notify: Arc<Notify>,
}

impl TestToolClient {
    fn new() -> Self {
        Self {
            notification_count: Arc::new(AtomicUsize::new(0)),
            notify: Arc::new(Notify::new()),
        }
    }
}

impl ClientHandler for TestToolClient {
    fn on_tool_list_changed(
        &self,
        _context: NotificationContext<RoleClient>,
    ) -> impl std::future::Future<Output = ()> + MaybeSendFuture + '_ {
        self.notification_count.fetch_add(1, Ordering::SeqCst);
        self.notify.notify_one();
        std::future::ready(())
    }
}

#[tokio::test]
async fn test_disable_enable_sends_tool_list_changed() {
    let server = TestToolServer::new();
    let trigger_disable = server.trigger_disable.clone();
    let trigger_enable = server.trigger_enable.clone();

    let client = TestToolClient::new();
    let notification_count = client.notification_count.clone();
    let client_notify = client.notify.clone();

    let (server_transport, client_transport) = tokio::io::duplex(4096);

    let server_handle = tokio::spawn(async move { server.serve(server_transport).await });
    let client_service = client.serve(client_transport).await.unwrap();

    let tools = client_service.peer().list_tools(None).await.unwrap();
    assert_eq!(tools.tools.len(), 2);

    trigger_disable.notify_one();
    tokio::time::timeout(std::time::Duration::from_secs(5), client_notify.notified())
        .await
        .expect("timed out waiting for tool_list_changed");
    assert_eq!(notification_count.load(Ordering::SeqCst), 1);

    let tools = client_service.peer().list_tools(None).await.unwrap();
    assert_eq!(tools.tools.len(), 1);
    assert_eq!(tools.tools[0].name, "tool_b");

    trigger_enable.notify_one();
    tokio::time::timeout(std::time::Duration::from_secs(5), client_notify.notified())
        .await
        .expect("timed out waiting for tool_list_changed");
    assert_eq!(notification_count.load(Ordering::SeqCst), 2);

    let tools = client_service.peer().list_tools(None).await.unwrap();
    assert_eq!(tools.tools.len(), 2);

    client_service.cancel().await.unwrap();
    server_handle.abort();
}
