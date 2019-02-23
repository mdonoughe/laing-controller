use futures::task::{self, Task};
use futures::Async;
use std::io::{self, Read, Write};
use std::sync::{Arc, Mutex, MutexGuard};
use tokio_io::{AsyncRead, AsyncWrite};

struct TransferPortState<T> {
    owner: usize,
    inner: T,
    rx_task: Option<Task>,
    tx_task: Option<Task>,
}

/// A wrapper around an AsyncRead+AsyncWrite that allows it to be reclaimed.
pub struct TransferPort<T> {
    state: Arc<Mutex<TransferPortState<T>>>,
}

/// A handle granting revokable access to an AsyncRead+AsyncWrite.
pub struct TransferPortHandle<T> {
    id: usize,
    state: Arc<Mutex<TransferPortState<T>>>,
}

impl<T> TransferPort<T> {
    pub fn new(inner: T) -> Self {
        Self {
            state: Arc::new(Mutex::new(TransferPortState {
                owner: 0,
                inner,
                rx_task: None,
                tx_task: None,
            })),
        }
    }

    /// Disconnect the current user's handle and get a new handle.
    ///
    /// Outstanding tasks will be notified, and further attempts to use the previous handle will return `BrokenPipe`.
    pub fn take(&self) -> TransferPortHandle<T> {
        let (rx_task, tx_task, handle) = {
            let mut state = self.state.lock().unwrap();
            state.owner += 1;
            (
                state.rx_task.take(),
                state.tx_task.take(),
                TransferPortHandle {
                    id: state.owner,
                    state: self.state.clone(),
                },
            )
        };
        if let Some(rx_task) = rx_task {
            rx_task.notify();
        }
        if let Some(tx_task) = tx_task {
            tx_task.notify();
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
    fn get_state(&self) -> Result<MutexGuard<TransferPortState<T>>, io::Error> {
        let state = self.state.lock().unwrap();
        if state.owner == self.id {
            Ok(state)
        } else {
            Err(io::Error::from(io::ErrorKind::BrokenPipe))
        }
    }
}

impl<T: Read> Read for TransferPortHandle<T> {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize, io::Error> {
        let mut state = self.get_state()?;
        match state.inner.read(buf) {
            Ok(r) => {
                state.rx_task = None;
                Ok(r)
            }
            Err(e) => match e.kind() {
                io::ErrorKind::WouldBlock => {
                    state.rx_task = Some(task::current());
                    Err(e)
                }
                _ => {
                    state.rx_task = None;
                    Err(e)
                }
            },
        }
    }
}

impl<T: AsyncRead> AsyncRead for TransferPortHandle<T> {}

impl<T: Write> Write for TransferPortHandle<T> {
    fn write(&mut self, buf: &[u8]) -> Result<usize, io::Error> {
        let mut state = self.get_state()?;
        match state.inner.write(buf) {
            Ok(r) => {
                state.tx_task = None;
                Ok(r)
            }
            Err(e) => match e.kind() {
                io::ErrorKind::WouldBlock => {
                    state.tx_task = Some(task::current());
                    Err(e)
                }
                _ => {
                    state.tx_task = None;
                    Err(e)
                }
            },
        }
    }

    fn flush(&mut self) -> Result<(), io::Error> {
        let mut state = self.get_state()?;
        match state.inner.flush() {
            Ok(r) => {
                state.tx_task = None;
                Ok(r)
            }
            Err(e) => match e.kind() {
                io::ErrorKind::WouldBlock => {
                    state.tx_task = Some(task::current());
                    Err(e)
                }
                _ => {
                    state.tx_task = None;
                    Err(e)
                }
            },
        }
    }
}

impl<T: AsyncWrite> AsyncWrite for TransferPortHandle<T> {
    fn shutdown(&mut self) -> Result<Async<()>, io::Error> {
        let mut state = self.get_state()?;
        match state.inner.shutdown() {
            Ok(Async::Ready(())) => {
                state.tx_task = None;
                Ok(Async::Ready(()))
            }
            Ok(Async::NotReady) => {
                state.tx_task = Some(task::current());
                Ok(Async::NotReady)
            }
            Err(e) => {
                state.tx_task = None;
                Err(e)
            }
        }
    }
}
