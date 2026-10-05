use std::{
    collections::HashMap,
    sync::{Arc, PoisonError, RwLock},
};

use futures::{Stream, StreamExt};
use tokio_stream::wrappers::ReceiverStream;

use crate::model::{ProgressNotificationParam, ProgressToken};
// A synchronous lock: it is never held across an `.await`, so a subscriber that is not
// keeping up only back-pressures its own token, and `ProgressSubscriber::drop` can
// unregister without spawning onto a runtime. Because that `drop` takes the lock, no caller
// code may run while it is held: senders are dropped (which can wake a task that drops its
// subscriber) and `Debug` output is written (into a caller's writer) only after release.
type Dispatcher =
    Arc<RwLock<HashMap<ProgressToken, tokio::sync::mpsc::Sender<ProgressNotificationParam>>>>;

/// A dispatcher for progress notifications.
#[derive(Clone, Default)]
pub struct ProgressDispatcher {
    pub(crate) dispatcher: Dispatcher,
}

impl std::fmt::Debug for ProgressDispatcher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Copy the tokens out first: the formatter writes into caller code.
        let subscriptions: Vec<ProgressToken> = self
            .dispatcher
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .keys()
            .cloned()
            .collect();
        f.debug_struct("ProgressDispatcher")
            .field("subscriptions", &subscriptions)
            .finish()
    }
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
        let replaced = self
            .dispatcher
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(progress_token.clone(), sender);
        drop(replaced);
        let receiver = ReceiverStream::new(receiver);
        ProgressSubscriber {
            progress_token,
            receiver,
            dispatcher: self.dispatcher.clone(),
        }
    }

    /// Unsubscribe from progress notifications for a specific token.
    pub async fn unsubscribe(&self, token: &ProgressToken) {
        let removed = self
            .dispatcher
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(token);
        drop(removed);
    }

    /// Clear all dispatcher.
    pub async fn clear(&self) {
        let removed = std::mem::take(
            &mut *self
                .dispatcher
                .write()
                .unwrap_or_else(PoisonError::into_inner),
        );
        drop(removed);
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
        // Only remove the entry if its receiver is closed. The token may have been
        // subscribed again since; that subscription's receiver is still open, so it
        // stays registered.
        let mut dispatcher = self
            .dispatcher
            .write()
            .unwrap_or_else(PoisonError::into_inner);
        let removed = if dispatcher
            .get(&self.progress_token)
            .is_some_and(|sender| sender.is_closed())
        {
            dispatcher.remove(&self.progress_token)
        } else {
            None
        };
        drop(dispatcher);
        drop(removed);
    }
}

#[cfg(test)]
mod tests {
    use std::{
        sync::{Mutex, mpsc::RecvTimeoutError},
        task::{Context, Wake, Waker},
        time::Duration,
    };

    use futures::FutureExt;

    use super::*;
    use crate::model::NumberOrString;

    /// A task that owns a subscriber and drops it as soon as it is woken, as a
    /// scheduler may synchronously do with a cancelled task.
    struct DropOnWake(Mutex<Option<ProgressSubscriber>>);

    impl DropOnWake {
        fn new(subscriber: ProgressSubscriber) -> Arc<Self> {
            Arc::new(Self(Mutex::new(Some(subscriber))))
        }

        /// Polls `subscriber` with this task's waker, so closing its channel wakes us.
        fn watch(self: &Arc<Self>, subscriber: &mut ProgressSubscriber) {
            let waker = Waker::from(self.clone());
            let poll = subscriber.poll_next_unpin(&mut Context::from_waker(&waker));
            assert!(poll.is_pending());
        }

        fn watch_own(self: &Arc<Self>) {
            let mut subscriber = self.0.lock().unwrap();
            self.watch(subscriber.as_mut().unwrap());
        }

        fn dropped(&self) -> bool {
            self.0.lock().unwrap().is_none()
        }
    }

    impl Wake for DropOnWake {
        fn wake(self: Arc<Self>) {
            self.wake_by_ref();
        }

        fn wake_by_ref(self: &Arc<Self>) {
            let subscriber = self.0.lock().unwrap().take();
            drop(subscriber);
        }
    }

    /// Runs `scenario` on its own thread: a deadlock blocks synchronously, so only
    /// another thread can notice it.
    fn assert_completes(scenario: impl FnOnce() + Send + 'static) {
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        let handle = std::thread::spawn(move || {
            scenario();
            let _ = done_tx.send(());
        });
        match done_rx.recv_timeout(Duration::from_secs(5)) {
            Ok(()) => {}
            Err(RecvTimeoutError::Timeout) => {
                panic!("deadlocked: a subscriber dropped under the lock could not take it")
            }
            Err(RecvTimeoutError::Disconnected) => {
                std::panic::resume_unwind(handle.join().unwrap_err())
            }
        }
    }

    fn subscribe(dispatcher: &ProgressDispatcher, token: &ProgressToken) -> ProgressSubscriber {
        dispatcher
            .subscribe(token.clone())
            .now_or_never()
            .expect("subscribe does not wait")
    }

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

    #[test]
    fn clear_tolerates_a_subscriber_dropped_on_wake() {
        assert_completes(|| {
            let dispatcher = ProgressDispatcher::new();
            let token = ProgressToken(NumberOrString::Number(1));
            let task = DropOnWake::new(subscribe(&dispatcher, &token));
            task.watch_own();

            dispatcher.clear().now_or_never().unwrap();
            assert!(task.dropped());
            assert!(dispatcher.dispatcher.read().unwrap().is_empty());
        });
    }

    #[test]
    fn unsubscribe_tolerates_a_subscriber_dropped_on_wake() {
        assert_completes(|| {
            let dispatcher = ProgressDispatcher::new();
            let token = ProgressToken(NumberOrString::Number(1));
            let task = DropOnWake::new(subscribe(&dispatcher, &token));
            task.watch_own();

            dispatcher.unsubscribe(&token).now_or_never().unwrap();
            assert!(task.dropped());
            assert!(dispatcher.dispatcher.read().unwrap().is_empty());
        });
    }

    #[test]
    fn resubscribe_tolerates_the_replaced_subscriber_dropped_on_wake() {
        assert_completes(|| {
            let dispatcher = ProgressDispatcher::new();
            let token = ProgressToken(NumberOrString::Number(1));
            let task = DropOnWake::new(subscribe(&dispatcher, &token));
            task.watch_own();

            let _replacement = subscribe(&dispatcher, &token);
            assert!(task.dropped());
            // The replaced subscriber's drop must not unregister the replacement.
            let registry = dispatcher.dispatcher.read().unwrap();
            assert!(
                registry
                    .get(&token)
                    .is_some_and(|sender| !sender.is_closed())
            );
        });
    }

    #[test]
    fn subscriber_drop_tolerates_another_subscriber_dropped_on_wake() {
        assert_completes(|| {
            let dispatcher = ProgressDispatcher::new();
            let first = ProgressToken(NumberOrString::Number(1));
            let second = ProgressToken(NumberOrString::Number(2));
            let mut subscriber = subscribe(&dispatcher, &first);
            let task = DropOnWake::new(subscribe(&dispatcher, &second));
            task.watch(&mut subscriber);

            drop(subscriber);
            assert!(task.dropped());
            assert!(dispatcher.dispatcher.read().unwrap().is_empty());
        });
    }

    /// A caller's writer that drops a subscriber it owns once the subscribed token is
    /// written, which a `Debug` that formats the registry in place does under the lock.
    struct DropOnWrite {
        subscriber: Option<ProgressSubscriber>,
        output: String,
    }

    impl std::fmt::Write for DropOnWrite {
        fn write_str(&mut self, text: &str) -> std::fmt::Result {
            if text.contains("drop-on-write") {
                drop(self.subscriber.take());
            }
            self.output.push_str(text);
            Ok(())
        }
    }

    #[test]
    fn debug_tolerates_a_subscriber_dropped_by_the_writer() {
        assert_completes(|| {
            use std::fmt::Write;

            let dispatcher = ProgressDispatcher::new();
            let token = ProgressToken(NumberOrString::String("drop-on-write".into()));
            let mut writer = DropOnWrite {
                subscriber: Some(subscribe(&dispatcher, &token)),
                output: String::new(),
            };

            write!(writer, "{dispatcher:?}").unwrap();
            assert!(writer.subscriber.is_none());
            assert_eq!(
                writer.output,
                r#"ProgressDispatcher { subscriptions: [ProgressToken(String("drop-on-write"))] }"#
            );
            assert!(dispatcher.dispatcher.read().unwrap().is_empty());
        });
    }
}
