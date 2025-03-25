//！ 写这一段原本是为了兼容官方sdk的tower-service这一套，但是想想还是算了，如果仅仅是请求-响应，都完成不了基本任务。要一个完成不了基本的任务的传输层干嘛？

use std::{collections::VecDeque, task::ready};

use crate::service::{RxJsonRpcMessage, ServiceRole, TxJsonRpcMessage};

use super::IntoTransport;
use futures::{Sink, Stream};
use tower_service::Service as TowerService;
pub enum TransportAdapterTower {}

impl<R, E, S> IntoTransport<R, E, TransportAdapterTower> for S
where
    S: TowerService<TxJsonRpcMessage<R>, Response = RxJsonRpcMessage<R>, Error = E>
        + Unpin
        + Send
        + 'static,
    R: ServiceRole,
    E: std::error::Error + Send + 'static,
    S::Future: Send + 'static,
    S::Response: Send + 'static,
{
    fn into_transport(
        self,
    ) -> (
        impl Sink<TxJsonRpcMessage<R>, Error = E> + Send + 'static,
        impl Stream<Item = RxJsonRpcMessage<R>> + Send + 'static,
    ) {
        let sink = TowerServiceSink {
            service: self,
            state: TowerServiceSinkState::Ready::<R, S>,
            rx_wakers: VecDeque::new(),
            tx_wakers: VecDeque::new(),
        };
        IntoTransport::<R, _, ()>::into_transport(sink)
    }
}

pin_project_lite::pin_project! {
    pub struct TowerServiceSink<R: ServiceRole, S: TowerService<TxJsonRpcMessage<R>>> {
        #[pin]
        service: S,
        #[pin]
        state: TowerServiceSinkState<R, S>,
        rx_wakers: VecDeque<std::task::Waker>,
        tx_wakers: VecDeque<std::task::Waker>,
    }
}
pin_project_lite::pin_project! {
    #[project = TowerServiceSinkStateProj]
    pub enum TowerServiceSinkState<R: ServiceRole, S: TowerService<TxJsonRpcMessage<R>>> {
        Ready,
        Sending {
            #[pin]
            fut: S::Future,
        },
        Yield {
            output: Option<S::Response>,
        }
    }
}

impl<R, S> Sink<TxJsonRpcMessage<R>> for TowerServiceSink<R, S>
where
    R: ServiceRole,
    S: TowerService<TxJsonRpcMessage<R>> + Unpin,
    S::Future: Send + 'static,
    S::Response: Send + 'static,
    S::Error: Send + 'static,
{
    type Error = S::Error;

    fn poll_ready(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), Self::Error>> {
        let mut this = self.as_mut().project();
        let service = this.service;
        ready!(service.get_mut().poll_ready(cx)?);
        match this.state.as_mut().project() {
            TowerServiceSinkStateProj::Ready => std::task::Poll::Ready(Ok(())),
            TowerServiceSinkStateProj::Sending { fut } => {
                ready!(fut.poll(cx))?;
                this.state.set(TowerServiceSinkState::Ready);
                std::task::Poll::Ready(Ok(()))
            }
            TowerServiceSinkStateProj::Yield { .. } => {
                this.state.set(TowerServiceSinkState::Ready);
                this.rx_wakers.drain(..).for_each(|waker| waker.wake());
                this.tx_wakers.push_back(cx.waker().clone());
                std::task::Poll::Pending
            }
        }
    }

    fn start_send(
        mut self: std::pin::Pin<&mut Self>,
        item: TxJsonRpcMessage<R>,
    ) -> Result<(), Self::Error> {
        let mut this = self.as_mut().project();
        let service = this.service;
        if let TxJsonRpcMessage::<R>::Request(req) = &item {
            let fut = service.get_mut().call(item);
            this.state.set(TowerServiceSinkState::Sending { fut });
        } else {
            tracing::debug!(message = ?item, "omit notification due to the transport layer limit");
            this.state.set(TowerServiceSinkState::Ready);
        }
        Ok(())
    }

    fn poll_flush(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), Self::Error>> {
        let mut this = self.as_mut().project();
        let service = this.service;
        ready!(service.get_mut().poll_ready(cx)?);
        loop {
            match this.state.as_mut().project() {
                TowerServiceSinkStateProj::Ready => return std::task::Poll::Ready(Ok(())),
                TowerServiceSinkStateProj::Sending { fut } => {
                    let output = ready!(fut.poll(cx))?;
                    this.state.set(TowerServiceSinkState::Yield {
                        output: Some(output),
                    });
                }
                TowerServiceSinkStateProj::Yield { .. } => {
                    this.state.set(TowerServiceSinkState::Ready);
                    this.rx_wakers.drain(..).for_each(|waker| waker.wake());
                    return std::task::Poll::Ready(Ok(()));
                }
            }
        }
    }

    fn poll_close(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), Self::Error>> {
        self.poll_flush(cx)
    }
}

impl<R, S> Stream for TowerServiceSink<R, S>
where
    R: ServiceRole,
    S: TowerService<TxJsonRpcMessage<R>, Response = RxJsonRpcMessage<R>>,
{
    type Item = S::Response;

    fn poll_next(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        let mut this = self.as_mut().project();
        match this.state.as_mut().project() {
            TowerServiceSinkStateProj::Ready => {
                this.rx_wakers.push_back(cx.waker().clone());
                std::task::Poll::Pending
            }
            TowerServiceSinkStateProj::Sending { .. } => {
                this.rx_wakers.push_back(cx.waker().clone());
                std::task::Poll::Pending
            }
            TowerServiceSinkStateProj::Yield { output } => {
                this.tx_wakers.drain(..).for_each(|waker| waker.wake());
                let output = output.take().expect("should have an output");
                this.state.set(TowerServiceSinkState::Ready);
                std::task::Poll::Ready(Some(output))
            }
        }
    }
}
