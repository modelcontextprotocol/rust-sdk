// cargo test --test test_handler_wrappers --features "client server"

mod common;

use std::sync::Arc;

use rmcp::{ClientHandler, ServerHandler};

use common::handlers::{TestClientHandler, TestServer};

#[test]
fn test_wrapped_server_handlers() {
    // This test asserts that, when T: ServerHandler, both Box<T> and Arc<T> also implement ServerHandler.
    fn accepts_server_handler<H: ServerHandler>(_handler: H) {}

    accepts_server_handler(Box::new(TestServer::new()));
    accepts_server_handler(Arc::new(TestServer::new()));
}

#[test]
fn test_wrapped_client_handlers() {
    // This test asserts that, when T: ClientHandler, both Box<T> and Arc<T> also implement ClientHandler.
    fn accepts_client_handler<H: ClientHandler>(_handler: H) {}

    let client = TestClientHandler::new(false, false);

    accepts_client_handler(Box::new(client.clone()));
    accepts_client_handler(Arc::new(client));
}
