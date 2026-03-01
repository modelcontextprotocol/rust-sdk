use futures::{Sink, Stream};
use std::{pin::Pin, task::Poll};

pub type PinnedFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

pub type PinnedLocalFuture<'a, T> = Pin<Box<dyn Future<Output = T> + 'a>>;

pub type PinnedStream<'a, T> = Pin<Box<dyn Stream<Item = T> + Send + 'a>>;

pub type PinnedLocalStream<'a, T> = Pin<Box<dyn Stream<Item = T> + 'a>>;

pub enum UnboundedSenderSinkError<T> {
    SendError(tokio::sync::mpsc::error::SendError<T>),
    Closed,
}

/// A simple [Sink] wrapper for Tokio's [tokio::sync::mpsc::UnboundedSender]
#[derive(Debug, Clone)]
pub struct UnboundedSenderSink<T> {
    sender: tokio::sync::mpsc::UnboundedSender<T>,
}

impl<T> UnboundedSenderSink<T> {
    pub fn new(sender: tokio::sync::mpsc::UnboundedSender<T>) -> Self {
        Self { sender }
    }
}

impl<T> Sink<T> for UnboundedSenderSink<T> {
    type Error = UnboundedSenderSinkError<T>;

    fn poll_ready(
        self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), Self::Error>> {
        let this = self.get_mut();
        if this.sender.is_closed() {
            Poll::Ready(Err(UnboundedSenderSinkError::Closed))
        } else {
            Poll::Ready(Ok(()))
        }
    }

    fn start_send(self: Pin<&mut Self>, item: T) -> Result<(), Self::Error> {
        let this = self.get_mut();
        match this.sender.send(item) {
            Ok(_) => Ok(()),
            Err(e) => Err(UnboundedSenderSinkError::SendError(e)),
        }
    }

    fn poll_flush(
        self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), Self::Error>> {
        // tokio's unbounded mpsc senders have no flushing required, since the
        // receiver is unbounded and will get all messages we send (unless we run
        // out of memory)
        Poll::Ready(Ok(()))
    }

    fn poll_close(
        self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), Self::Error>> {
        // Like `poll_flush`, there is nothing to wait on here. A single
        // call to `mpsc_sender.send(...)` is immediate from the perspective
        // of the sender
        Poll::Ready(Ok(()))
    }
}
