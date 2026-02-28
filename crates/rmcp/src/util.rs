use std::pin::Pin;

pub type PinnedFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

pub type PinnedLocalFuture<'a, T> = Pin<Box<dyn Future<Output = T> + 'a>>;
