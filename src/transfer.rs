use std::io;
use std::pin::Pin;
use std::sync::{Arc, Mutex, MutexGuard};
use std::task::{Context, Poll, Waker};

use pin_project::pin_project;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

#[pin_project]
struct TransferPortState<T> {
    owner: usize,
    #[pin]
    inner: T,
    rx_task: Option<Waker>,
    tx_task: Option<Waker>,
}

/// A wrapper type that allows ownership of an async read+write object to be forcibly transferred.
///
/// This is a workaround for problems with tokio-modbus and tokio-serial. If a read timeout occurs,
/// tokio-modbus does not recover, so we need to reset. However, disconnecting and dropping the
/// modbus `Context` object does not close the serial port handle, so trying to open the serial port
/// again for the new context will fail.
///
/// When `take` is called, a `TransferPortHandle` implementing `AsyncRead` and `AsyncWrite` is
/// returned. Calling `take` again will invalidate the old handle, causing all its pending futures
/// to immediately fail, before returning a new handle.
pub struct TransferPort<T> {
    state: Arc<Mutex<Pin<Box<TransferPortState<T>>>>>,
}

/// A handle granting revokable access to an AsyncRead+AsyncWrite.
pub struct TransferPortHandle<T> {
    id: usize,
    state: Arc<Mutex<Pin<Box<TransferPortState<T>>>>>,
}

impl<T> TransferPort<T> {
    pub fn new(inner: T) -> Self {
        Self {
            state: Arc::new(Mutex::new(Box::pin(TransferPortState {
                owner: 0,
                inner,
                rx_task: None,
                tx_task: None,
            }))),
        }
    }

    /// Disconnect the current user's handle and get a new handle.
    ///
    /// Outstanding tasks will be notified, and further attempts to use the previous handle will return `BrokenPipe`.
    pub fn take(&self) -> TransferPortHandle<T> {
        let (rx_task, tx_task, handle) = {
            let mut state = self.state.lock().unwrap();
            let state = state.as_mut().project();
            *state.owner += 1;
            (
                state.rx_task.take(),
                state.tx_task.take(),
                TransferPortHandle {
                    id: *state.owner,
                    state: self.state.clone(),
                },
            )
        };
        if let Some(rx_task) = rx_task {
            rx_task.wake();
        }
        if let Some(tx_task) = tx_task {
            tx_task.wake();
        }
        handle
    }
}

impl<T> Clone for TransferPort<T> {
    fn clone(&self) -> Self {
        Self {
            state: self.state.clone(),
        }
    }
}

impl<T> TransferPortHandle<T> {
    fn get_state(&self) -> Result<MutexGuard<Pin<Box<TransferPortState<T>>>>, io::Error> {
        let state = self.state.lock().unwrap();
        if state.owner == self.id {
            Ok(state)
        } else {
            Err(io::Error::from(io::ErrorKind::BrokenPipe))
        }
    }
}

impl<T: AsyncRead> AsyncRead for TransferPortHandle<T> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let mut state = self.get_state()?;
        let state = state.as_mut().project();
        match state.inner.poll_read(cx, buf) {
            Poll::Ready(r) => {
                *state.rx_task = None;
                Poll::Ready(r)
            }
            Poll::Pending => {
                *state.rx_task = Some(cx.waker().clone());
                Poll::Pending
            }
        }
    }
}

impl<T: AsyncWrite> AsyncWrite for TransferPortHandle<T> {
    fn poll_write(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<Result<usize, io::Error>> {
        let mut state = self.get_state()?;
        let state = state.as_mut().project();
        match state.inner.poll_write(cx, buf) {
            Poll::Ready(r) => {
                *state.tx_task = None;
                Poll::Ready(r)
            }
            Poll::Pending => {
                *state.tx_task = Some(cx.waker().clone());
                Poll::Pending
            }
        }
    }

    fn poll_flush(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), io::Error>> {
        let mut state = self.get_state()?;
        let state = state.as_mut().project();
        match state.inner.poll_flush(cx) {
            Poll::Ready(r) => {
                *state.tx_task = None;
                Poll::Ready(r)
            }
            Poll::Pending => {
                *state.tx_task = Some(cx.waker().clone());
                Poll::Pending
            }
        }
    }

    fn poll_shutdown(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), io::Error>> {
        let mut state = self.get_state()?;
        let state = state.as_mut().project();
        match state.inner.poll_shutdown(cx) {
            Poll::Ready(r) => {
                *state.tx_task = None;
                Poll::Ready(r)
            }
            Poll::Pending => {
                *state.tx_task = Some(cx.waker().clone());
                Poll::Pending
            }
        }
    }
}

impl<T> std::fmt::Debug for TransferPortHandle<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TransferPortHandle").finish()
    }
}
