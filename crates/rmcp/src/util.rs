use futures::Stream;
use std::pin::Pin;

pub type PinnedFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

pub type PinnedLocalFuture<'a, T> = Pin<Box<dyn Future<Output = T> + 'a>>;

pub type PinnedStream<'a, T> = Pin<Box<dyn Stream<Item = T> + Send + 'a>>;

pub type PinnedLocalStream<'a, T> = Pin<Box<dyn Stream<Item = T> + 'a>>;
