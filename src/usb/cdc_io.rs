//! embedded_io_async wrapper for CDC-ACM Receiver/Sender.
//!
//! Provides Read/Write implementations for CDC packet-based API.

use embassy_usb::class::cdc_acm::{Receiver, Sender};
use embassy_usb::driver::Driver;
use embedded_io_async::{ErrorType, Read, Write};

/// Error type for CDC I/O operations.
#[derive(Debug, Clone, Copy)]
pub struct CdcError;

impl embedded_io::Error for CdcError {
    fn kind(&self) -> embedded_io::ErrorKind {
        embedded_io::ErrorKind::Other
    }
}

/// Wrapper around CDC Receiver that implements embedded_io_async::Read.
pub struct CdcReader<'d, D: Driver<'d>> {
    inner: Receiver<'d, D>,
}

impl<'d, D: Driver<'d>> CdcReader<'d, D> {
    pub fn new(inner: Receiver<'d, D>) -> Self {
        Self { inner }
    }
}

impl<'d, D: Driver<'d>> ErrorType for CdcReader<'d, D> {
    type Error = CdcError;
}

impl<'d, D: Driver<'d>> Read for CdcReader<'d, D> {
    async fn read(&mut self, buf: &mut [u8]) -> Result<usize, Self::Error> {
        // Wait for DTR (Data Terminal Ready) before reading
        self.inner.wait_connection().await;

        match self.inner.read_packet(buf).await {
            Ok(n) => Ok(n),
            Err(_) => Err(CdcError),
        }
    }
}

/// Wrapper around CDC Sender that implements embedded_io_async::Write.
pub struct CdcWriter<'d, D: Driver<'d>> {
    inner: Sender<'d, D>,
}

impl<'d, D: Driver<'d>> CdcWriter<'d, D> {
    pub fn new(inner: Sender<'d, D>) -> Self {
        Self { inner }
    }
}

impl<'d, D: Driver<'d>> ErrorType for CdcWriter<'d, D> {
    type Error = CdcError;
}

impl<'d, D: Driver<'d>> Write for CdcWriter<'d, D> {
    async fn write(&mut self, buf: &[u8]) -> Result<usize, Self::Error> {
        // Wait for DTR before writing
        self.inner.wait_connection().await;

        match self.inner.write_packet(buf).await {
            Ok(()) => Ok(buf.len()),
            Err(_) => Err(CdcError),
        }
    }

    async fn flush(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }
}
