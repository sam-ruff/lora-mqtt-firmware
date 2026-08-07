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

        // write_packet errors outright on data longer than the 64-byte USB
        // endpoint packet, which silently dropped every frame over 64 bytes.
        // Cap the write and let write_all loop over the rest.
        //
        // The cap is one UNDER the endpoint packet size: a bulk transfer only
        // terminates on a short packet, so a frame whose tail lands exactly
        // on the 64-byte boundary would sit undelivered in the host's CDC
        // driver until unrelated later bytes flush it (seen on hardware as a
        // response of exactly 64 encoded bytes never arriving).
        let n = buf.len().min(self.inner.max_packet_size() as usize - 1);
        match self.inner.write_packet(&buf[..n]).await {
            Ok(()) => Ok(n),
            Err(_) => Err(CdcError),
        }
    }

    async fn flush(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }
}
