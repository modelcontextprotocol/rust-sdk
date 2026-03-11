use tokio::io::{AsyncRead as TokioAsyncRead, AsyncWrite as TokioAsyncWrite};
use tokio_util::compat::{FuturesAsyncReadCompatExt, FuturesAsyncWriteCompatExt};

use crate::{
    service::ServiceRole,
    transport::{
        Transport,
        async_rw::AsyncRwTransport,
        child_process::runner::{ChildProcess, ChildProcessControl},
    },
};

#[derive(thiserror::Error, Debug)]
pub enum ChildProcessTransportError {
    #[error("Missing stdout")]
    MissingStdout,
    #[error("Missing stdin")]
    MissingStdin,
}

pub struct ChildProcessTransport<R: ServiceRole> {
    _child: Box<dyn ChildProcessControl + Send>,
    framed_transport: AsyncRwTransport<
        R,
        Box<dyn TokioAsyncRead + Unpin + Send>,
        Box<dyn TokioAsyncWrite + Unpin + Send>,
    >,
}

impl<R> ChildProcessTransport<R>
where
    R: ServiceRole,
{
    pub fn new(child: ChildProcess) -> Result<Self, ChildProcessTransportError> {
        let (stdout, stdin, _stderr, control) = child.split();

        let framed_transport: AsyncRwTransport<R, _, _> = AsyncRwTransport::new(
            Box::new(
                stdout
                    .ok_or(ChildProcessTransportError::MissingStdout)?
                    .compat(),
            ) as Box<dyn TokioAsyncRead + Unpin + Send>,
            Box::new(
                stdin
                    .ok_or(ChildProcessTransportError::MissingStdin)?
                    .compat_write(),
            ) as Box<dyn TokioAsyncWrite + Unpin + Send>,
        );

        Ok(Self {
            _child: control,
            framed_transport,
        })
    }
}

impl<R> Transport<R> for ChildProcessTransport<R>
where
    R: ServiceRole,
{
    type Error = std::io::Error;

    fn send(
        &mut self,
        item: crate::service::TxJsonRpcMessage<R>,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send + 'static {
        self.framed_transport.send(item)
    }

    fn receive(
        &mut self,
    ) -> impl Future<Output = Option<crate::service::RxJsonRpcMessage<R>>> + Send {
        self.framed_transport.receive()
    }

    fn close(&mut self) -> impl Future<Output = Result<(), Self::Error>> + Send {
        self.framed_transport.close()
    }
}
