use std::{collections::HashMap, sync::Arc};

use futures::{Stream, StreamExt};
use tokio::sync::{RwLock, broadcast};
use tokio_stream::wrappers::BroadcastStream;

use crate::{
    model::{ProgressNotificationParam, ProgressToken},
    util::PinnedStream,
};

/// A dispatcher for progress notifications.
///
/// See [ProgressNotificationParam] and [ProgressToken] for more details on
/// how progress is dispatched to a particular listener.
#[derive(Debug, Clone)]
pub struct ProgressDispatcher {
    /// A channel of any progress notification. Subscribers will filter
    /// on this channel.
    pub(crate) any_progress_notification_tx: broadcast::Sender<ProgressNotificationParam>,
    pub(crate) unsubscribe_tx: broadcast::Sender<ProgressToken>,
    pub(crate) unsubscribe_all_tx: broadcast::Sender<()>,
}

impl ProgressDispatcher {
    const CHANNEL_SIZE: usize = 16;
    pub fn new() -> Self {
        // Note that channel size is per-receiver for broadcast channel. It is up to the receiver to
        // keep up with the notifications to avoid missing any (via propper polling)
        let (any_progress_notification_tx, _) = broadcast::channel(Self::CHANNEL_SIZE);
        let (unsubscribe_tx, _) = broadcast::channel(Self::CHANNEL_SIZE);
        let (unsubscribe_all_tx, _) = broadcast::channel(Self::CHANNEL_SIZE);
        Self {
            any_progress_notification_tx,
            unsubscribe_tx,
            unsubscribe_all_tx,
        }
    }

    /// Handle a progress notification by sending it to the appropriate subscriber
    pub async fn handle_notification(&self, notification: ProgressNotificationParam) {
        // Broadcast the notification to all subscribers. Interested subscribers
        // will filter on their end.
        // ! Note that this implementaiton is very stateless and simple, we cannot
        // ! easily inspect which subscribers are interested in which notifications.
        // ! However, the stateless-ness and simplicity is also a plus!
        // ! Cleanup becomes much easier. Just drop the `ProgressSubscriber`.
        match self.any_progress_notification_tx.send(notification) {
            Ok(_) => {}
            Err(_) => {
                // This error only happens if there are no active receivers of the `broadcast` channel.
                // Silent error.
            }
        }
    }

    /// Subscribe to progress notifications for a specific token.
    ///
    /// If you drop the returned `ProgressSubscriber`, it will automatically unsubscribe from notifications for that token.
    pub async fn subscribe(&self, progress_token: ProgressToken) -> ProgressSubscriber {
        // First, set up the unsubscribe listeners. This will fuse the notifiaction stream below.
        let progress_token_clone = progress_token.clone();
        let unsub_this_token_rx = BroadcastStream::new(self.unsubscribe_tx.subscribe()).filter_map(
            move |token| {
                let progress_token_clone = progress_token_clone.clone();
                async move {
                match token {
                    Ok(token) => {
                        if token == progress_token_clone {
                            Some(())
                        } else {
                            None
                        }
                    }
                    Err(e) => {
                        // An error here means the broadcast stream did not receive values quick enough and
                        // and we missed some notification. This implies there are notifications
                        // we missed, but we cannot assume they were for us :(
                        tracing::warn!(
                            "Error receiving unsubscribe notification from broadcast channel: {e}"
                        );
                        None
                    }
                }
            }
            },
        );
        let unsub_any_token_tx =
            BroadcastStream::new(self.unsubscribe_all_tx.subscribe()).map(|_| {
                // Any reception of a result here indicates we should unsubscribe,
                // regardless of if we received an `Ok(())` or an `Err(_)` (which
                // indicates the broadcast receiver lagged behind)
                ()
            });
        let unsub_fut = futures::stream::select(unsub_this_token_rx, unsub_any_token_tx)
            .boxed()
            .into_future(); // If the unsub streams end, this will cause unsubscription from the subscriber below.

        // Now setup the notification stream. We will receive all notifications and only forward progress notifications
        // for the token we're interested in.
        let progress_token_clone = progress_token.clone();
        let receiver = BroadcastStream::new(self.any_progress_notification_tx.subscribe())
            .filter_map(move |notification| {
                let progress_token_clone = progress_token_clone.clone();
                async move {
                    // We need to kneed-out the broadcast receive error type here.
                    match notification {
                        Ok(notification) => {
                            let token = notification.progress_token.clone();
                            if token == progress_token_clone {
                                Some(notification)
                            } else {
                                None
                            }
                        }
                        Err(e) => {
                            tracing::warn!(
                                "Error receiving progress notification from broadcast channel: {e}"
                            );
                            None
                        }
                    }
                }
            })
            // Fuse this stream so it stops once we receive an unsubscribe notification from the stream
            // created above
            .take_until(unsub_fut)
            .boxed();

        ProgressSubscriber {
            progress_token,
            receiver,
        }
    }

    /// Unsubscribe from progress notifications for a specific token.
    pub fn unsubscribe(&self, token: ProgressToken) {
        // The only error defined is if there are no listeners, which is fine. Ignore the result.
        let _ = self.unsubscribe_tx.send(token);
    }

    /// Clear all dispatcher.
    pub fn clear(&self) {
        // The only error defined is if there are no listeners, which is fine. Ignore the result.
        let _ = self.unsubscribe_all_tx.send(());
    }
}

pub struct ProgressSubscriber {
    pub(crate) progress_token: ProgressToken,
    pub(crate) receiver: PinnedStream<'static, ProgressNotificationParam>,
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
