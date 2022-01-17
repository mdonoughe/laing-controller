use std::future::Future;
use std::io;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::{Duration};
use pin_project::pin_project;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::time::Sleep;

/// A wrapper around an AsyncRead+AsyncWrite to add read timeouts.
#[pin_project]
pub struct TimeoutPort<T> {
    #[pin]
    inner: T,
    timeout: Duration,
    timeout_delay: Option<Pin<Box<Sleep>>>,
}

impl<T> TimeoutPort<T> {
    pub fn new(inner: T, timeout: Duration) -> Self {
        Self {
            inner,
            timeout,
            timeout_delay: None,
        }
    }
}

impl<T: AsyncRead> AsyncRead for TimeoutPort<T> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.project();
        match this.inner.poll_read(cx, buf) {
            Poll::Ready(r) => {
                *this.timeout_delay = None;
                Poll::Ready(r)
            }
            Poll::Pending => {
                let timeout = *this.timeout;
                match this
                    .timeout_delay
                    .get_or_insert_with(|| Box::pin(tokio::time::sleep(timeout)))
                    .as_mut()
                    .poll(cx)
                {
                    Poll::Pending => Poll::Pending,
                    _ => {
                        *this.timeout_delay = None;
                        Poll::Ready(Err(io::Error::from(io::ErrorKind::TimedOut)))
                    }
                }
            }
        }
    }
}

impl<T: AsyncWrite> AsyncWrite for TimeoutPort<T> {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<Result<usize, io::Error>> {
        let this = self.project();
        this.inner.poll_write(cx, buf)
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), io::Error>> {
        let this = self.project();
        this.inner.poll_flush(cx)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), io::Error>> {
        let this = self.project();
        this.inner.poll_shutdown(cx)
    }
}
