use std::{
    collections::HashMap,
    sync::{Arc, PoisonError, RwLock},
};

use futures::{Stream, StreamExt};
use tokio_stream::wrappers::ReceiverStream;

use crate::model::{ProgressNotificationParam, ProgressToken};
// A synchronous lock: it is never held across an `.await`, so a subscriber that is not
// keeping up only back-pressures its own token, and `ProgressSubscriber::drop` can
// unregister without spawning onto a runtime.
type Dispatcher =
    Arc<RwLock<HashMap<ProgressToken, tokio::sync::mpsc::Sender<ProgressNotificationParam>>>>;

/// A dispatcher for progress notifications.
#[derive(Debug, Clone, Default)]
pub struct ProgressDispatcher {
    pub(crate) dispatcher: Dispatcher,
}

impl ProgressDispatcher {
    const CHANNEL_SIZE: usize = 16;
    pub fn new() -> Self {
        Self::default()
    }

    /// Handle a progress notification by sending it to the appropriate subscriber
    pub async fn handle_notification(&self, notification: ProgressNotificationParam) {
        let sender = self
            .dispatcher
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&notification.progress_token)
            .cloned();
        if let Some(sender) = sender {
            let send_result = sender.send(notification).await;
            if let Err(e) = send_result {
                tracing::warn!("Failed to send progress notification: {e}");
            }
        }
    }

    /// Subscribe to progress notifications for a specific token.
    ///
    /// If you drop the returned `ProgressSubscriber`, it will automatically unsubscribe from notifications for that token.
    pub async fn subscribe(&self, progress_token: ProgressToken) -> ProgressSubscriber {
        let (sender, receiver) = tokio::sync::mpsc::channel(Self::CHANNEL_SIZE);
        self.dispatcher
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(progress_token.clone(), sender);
        let receiver = ReceiverStream::new(receiver);
        ProgressSubscriber {
            progress_token,
            receiver,
            dispatcher: self.dispatcher.clone(),
        }
    }

    /// Unsubscribe from progress notifications for a specific token.
    pub async fn unsubscribe(&self, token: &ProgressToken) {
        self.dispatcher
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(token);
    }

    /// Clear all dispatcher.
    pub async fn clear(&self) {
        self.dispatcher
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .clear();
    }
}

pub struct ProgressSubscriber {
    pub(crate) progress_token: ProgressToken,
    pub(crate) receiver: ReceiverStream<ProgressNotificationParam>,
    pub(crate) dispatcher: Dispatcher,
}

impl ProgressSubscriber {
    pub fn progress_token(&self) -> &ProgressToken {
        &self.progress_token
    }
}

impl Stream for ProgressSubscriber {
    type Item = ProgressNotificationParam;

    fn poll_next(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        self.receiver.poll_next_unpin(cx)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.receiver.size_hint()
    }
}

impl Drop for ProgressSubscriber {
    fn drop(&mut self) {
        self.receiver.close();
        // Only remove the entry if it still belongs to this subscriber: the token may
        // have been subscribed again since, and that subscription must stay registered.
        let mut dispatcher = self
            .dispatcher
            .write()
            .unwrap_or_else(PoisonError::into_inner);
        if dispatcher
            .get(&self.progress_token)
            .is_some_and(|sender| sender.is_closed())
        {
            dispatcher.remove(&self.progress_token);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::NumberOrString;

    #[test]
    fn dropping_a_subscriber_unregisters_it_without_a_runtime() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("build runtime");
        let dispatcher = ProgressDispatcher::new();
        let token = ProgressToken(NumberOrString::Number(1));
        let subscriber = runtime.block_on(dispatcher.subscribe(token.clone()));
        assert!(dispatcher.dispatcher.read().unwrap().contains_key(&token));

        // Dropped outside of any Tokio runtime context.
        drop(subscriber);
        assert!(dispatcher.dispatcher.read().unwrap().is_empty());
    }
}
