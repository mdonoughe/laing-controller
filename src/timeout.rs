use futures::future::Future;
use futures::Async;
use std::io::{self, Read, Write};
use std::time::{Duration, Instant};
use tokio_io::{AsyncRead, AsyncWrite};
use tokio_timer::Delay;

/// A wrapper around an AsyncRead+AsyncWrite to add read timeouts.
pub struct TimeoutPort<T> {
    inner: T,
    timeout: Duration,
    timeout_delay: Option<Delay>,
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

impl<T: Read> Read for TimeoutPort<T> {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize, io::Error> {
        match self.inner.read(buf) {
            Ok(r) => {
                self.timeout_delay = None;
                Ok(r)
            }
            Err(e) => match e.kind() {
                io::ErrorKind::WouldBlock => {
                    let timeout = self.timeout;
                    match self
                        .timeout_delay
                        .get_or_insert_with(|| Delay::new(Instant::now() + timeout))
                        .poll()
                    {
                        Ok(Async::NotReady) => Err(e),
                        _ => {
                            self.timeout_delay = None;
                            Err(io::Error::from(io::ErrorKind::TimedOut))
                        }
                    }
                }
                _ => {
                    self.timeout_delay = None;
                    Err(e)
                }
            },
        }
    }
}

impl<T: AsyncRead> AsyncRead for TimeoutPort<T> {}

impl<T: Write> Write for TimeoutPort<T> {
    fn write(&mut self, buf: &[u8]) -> Result<usize, io::Error> {
        self.inner.write(buf)
    }
    fn flush(&mut self) -> Result<(), io::Error> {
        self.inner.flush()
    }
}

impl<T: AsyncWrite> AsyncWrite for TimeoutPort<T> {
    fn shutdown(&mut self) -> Result<Async<()>, io::Error> {
        self.inner.shutdown()
    }
}
