use std::rc::Rc;

use rmcp::ClientHandler;

struct LocalClientHandler {
    _state: Rc<()>,
}

impl ClientHandler for LocalClientHandler {}

#[test]
fn client_handler_accepts_non_send_sync_state_with_local_feature() {
    fn assert_client_handler<T: ClientHandler>() {}

    assert_client_handler::<LocalClientHandler>();
}
