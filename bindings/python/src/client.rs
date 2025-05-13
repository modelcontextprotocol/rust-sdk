//! Python bindings client handler implementation.
//!
//! This module provides the `PyClientHandler` struct, which implements the `ClientHandler` trait for use in Python bindings.
//! It allows sending and receiving messages, managing peers, and listing root messages in a client context.
//!
//! # Examples
//!
//! ```rust
//! use bindings::python::client::PyClientHandler;
//! let handler = PyClientHandler::new();
//! ```
#![allow(non_local_definitions)]

use rmcp::service::{RoleClient, RequestContext};
use rmcp::ClientHandler;
use rmcp::model::{CreateMessageRequestParam, SamplingMessage, Role, Content, CreateMessageResult, ListRootsResult};
use std::future::Future;
use rmcp::service::Peer;

/// A client handler for use in Python bindings.
///
/// This struct manages an optional peer and implements the `ClientHandler` trait.
#[derive(Clone)]
pub struct PyClientHandler {
    /// The current peer associated with this handler, if any.
    peer: Option<Peer<RoleClient>>,
}

impl PyClientHandler {
    /// Creates a new `PyClientHandler` with no peer set.
    ///
    /// # Examples
    ///
    /// ```rust
    /// let handler = PyClientHandler::new();
    /// assert!(handler.get_peer().is_none());
    /// ```
    pub fn new() -> Self {
        Self {
            peer: None,
        }
    }
}

impl ClientHandler for PyClientHandler {
    /// Creates a message in response to a request.
    ///
    /// # Parameters
    /// - `_params`: The parameters for the message creation request.
    /// - `_context`: The request context.
    ///
    /// # Returns
    /// A future resolving to a `CreateMessageResult` containing the created message.
    ///
    /// # Examples
    ///
    /// ```rust
    /// // Usage in async context
    /// // let result = handler.create_message(params, context).await;
    /// ```
    fn create_message(
        &self,
        _params: CreateMessageRequestParam,
        _context: RequestContext<RoleClient>,
    ) -> impl Future<Output = Result<CreateMessageResult, rmcp::Error>> + Send + '_ {
        // Create a default message for now
        let message = SamplingMessage {
            role: Role::Assistant,
            content: Content::text("".to_string()),
        };
        let result = CreateMessageResult {
            model: "default-model".to_string(),
            stop_reason: None,
            message,
        };
        std::future::ready(Ok(result))
    }

    /// Lists root messages for the client.
    ///
    /// # Parameters
    /// - `_context`: The request context.
    ///
    /// # Returns
    /// A future resolving to a `ListRootsResult` containing the list of root messages.
    ///
    /// # Examples
    ///
    /// ```rust
    /// // Usage in async context
    /// // let roots = handler.list_roots(context).await;
    /// ```
    fn list_roots(
        &self,
        _context: RequestContext<RoleClient>,
    ) -> impl Future<Output = Result<ListRootsResult, rmcp::Error>> + Send + '_ {
        // Return empty list for now
        std::future::ready(Ok(ListRootsResult { roots: vec![] }))
    }

    /// Returns the current peer, if any.
    ///
    /// # Returns
    /// An `Option<Peer<RoleClient>>` containing the current peer if set.
    ///
    /// # Examples
    ///
    /// ```rust
    /// let peer = handler.get_peer();
    /// ```
    fn get_peer(&self) -> Option<Peer<RoleClient>> {
        self.peer.clone()
    }

    /// Sets the current peer.
    ///
    /// # Parameters
    /// - `peer`: The peer to set for this handler.
    ///
    /// # Examples
    ///
    /// ```rust
    /// handler.set_peer(peer);
    /// ```
    fn set_peer(&mut self, peer: Peer<RoleClient>) {
        self.peer = Some(peer);
    }
}