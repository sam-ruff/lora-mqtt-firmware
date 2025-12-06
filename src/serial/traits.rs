//! Serial port trait for abstraction and testability
//!
//! This trait defines the interface for serial port operations,
//! allowing the actual UART driver to be swapped with a mock for testing.

use core::future::Future;

/// Errors that can occur during serial operations
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SerialError {
    /// Framing error in received data
    FramingError,
    /// Buffer overflow
    OverflowError,
    /// Operation timed out
    Timeout,
    /// Write error
    WriteError,
}

/// Abstract serial port interface for testability
///
/// This trait allows the serial reader to work with either the real UART
/// hardware driver or a mock implementation for testing.
pub trait SerialPort {
    /// Read bytes into buffer
    ///
    /// Returns the number of bytes actually read. May return fewer bytes
    /// than the buffer size if data is not immediately available.
    fn read(&mut self, buf: &mut [u8]) -> impl Future<Output = Result<usize, SerialError>>;

    /// Write bytes from buffer
    fn write(&mut self, data: &[u8]) -> impl Future<Output = Result<(), SerialError>>;

    /// Flush the write buffer
    fn flush(&mut self) -> impl Future<Output = Result<(), SerialError>>;
}

#[cfg(test)]
pub mod mock {
    //! Mock serial port for testing

    use super::*;
    use crate::config::protocol::MAX_FRAME_SIZE;
    use core::cell::RefCell;
    use heapless::Vec;

    /// Mock serial port for unit testing
    pub struct MockSerialPort {
        /// Data queued to be returned by read()
        rx_buffer: RefCell<Vec<u8, { MAX_FRAME_SIZE * 4 }>>,
        /// Data written via write()
        tx_buffer: RefCell<Vec<u8, { MAX_FRAME_SIZE * 4 }>>,
        /// Error to return on next read
        next_read_error: RefCell<Option<SerialError>>,
        /// Error to return on next write
        next_write_error: RefCell<Option<SerialError>>,
    }

    impl MockSerialPort {
        /// Create a new mock serial port
        pub fn new() -> Self {
            Self {
                rx_buffer: RefCell::new(Vec::new()),
                tx_buffer: RefCell::new(Vec::new()),
                next_read_error: RefCell::new(None),
                next_write_error: RefCell::new(None),
            }
        }

        /// Queue data to be returned by read()
        pub fn queue_rx_data(&self, data: &[u8]) {
            let _ = self.rx_buffer.borrow_mut().extend_from_slice(data);
        }

        /// Get all data written via write()
        pub fn get_tx_data(&self) -> Vec<u8, { MAX_FRAME_SIZE * 4 }> {
            self.tx_buffer.borrow().clone()
        }

        /// Clear the TX buffer
        pub fn clear_tx_buffer(&self) {
            self.tx_buffer.borrow_mut().clear();
        }

        /// Set an error to be returned by the next read() call
        pub fn set_next_read_error(&self, error: SerialError) {
            *self.next_read_error.borrow_mut() = Some(error);
        }

        /// Set an error to be returned by the next write() call
        pub fn set_next_write_error(&self, error: SerialError) {
            *self.next_write_error.borrow_mut() = Some(error);
        }
    }

    impl Default for MockSerialPort {
        fn default() -> Self {
            Self::new()
        }
    }

    impl SerialPort for MockSerialPort {
        async fn read(&mut self, buf: &mut [u8]) -> Result<usize, SerialError> {
            if let Some(error) = self.next_read_error.borrow_mut().take() {
                return Err(error);
            }

            let mut rx = self.rx_buffer.borrow_mut();
            if rx.is_empty() {
                // No data available - in real impl this would block
                return Ok(0);
            }

            // Read up to buf.len() bytes
            let count = core::cmp::min(buf.len(), rx.len());
            buf[..count].copy_from_slice(&rx[..count]);

            // Remove read bytes from buffer (shift remaining)
            let remaining: Vec<u8, { MAX_FRAME_SIZE * 4 }> =
                rx[count..].iter().copied().collect();
            *rx = remaining;

            Ok(count)
        }

        async fn write(&mut self, data: &[u8]) -> Result<(), SerialError> {
            if let Some(error) = self.next_write_error.borrow_mut().take() {
                return Err(error);
            }

            self.tx_buffer
                .borrow_mut()
                .extend_from_slice(data)
                .map_err(|_| SerialError::OverflowError)?;

            Ok(())
        }

        async fn flush(&mut self) -> Result<(), SerialError> {
            Ok(())
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn test_mock_read() {
            let mut port = MockSerialPort::new();

            futures::executor::block_on(async {
                port.queue_rx_data(&[0x01, 0x02, 0x03]);

                let mut buf = [0u8; 10];
                let count = port.read(&mut buf).await.unwrap();

                assert_eq!(count, 3);
                assert_eq!(&buf[..3], &[0x01, 0x02, 0x03]);
            });
        }

        #[test]
        fn test_mock_partial_read() {
            let mut port = MockSerialPort::new();

            futures::executor::block_on(async {
                port.queue_rx_data(&[0x01, 0x02, 0x03, 0x04, 0x05]);

                // Read only 2 bytes
                let mut buf = [0u8; 2];
                let count = port.read(&mut buf).await.unwrap();
                assert_eq!(count, 2);
                assert_eq!(&buf, &[0x01, 0x02]);

                // Read remaining
                let mut buf = [0u8; 10];
                let count = port.read(&mut buf).await.unwrap();
                assert_eq!(count, 3);
                assert_eq!(&buf[..3], &[0x03, 0x04, 0x05]);
            });
        }

        #[test]
        fn test_mock_write() {
            let mut port = MockSerialPort::new();

            futures::executor::block_on(async {
                port.write(&[0x01, 0x02]).await.unwrap();
                port.write(&[0x03, 0x04]).await.unwrap();

                let written = port.get_tx_data();
                assert_eq!(written.as_slice(), &[0x01, 0x02, 0x03, 0x04]);
            });
        }

        #[test]
        fn test_mock_read_error() {
            let mut port = MockSerialPort::new();

            futures::executor::block_on(async {
                port.set_next_read_error(SerialError::FramingError);

                let mut buf = [0u8; 10];
                let result = port.read(&mut buf).await;
                assert_eq!(result, Err(SerialError::FramingError));

                // Error should be cleared
                port.queue_rx_data(&[0x01]);
                let count = port.read(&mut buf).await.unwrap();
                assert_eq!(count, 1);
            });
        }
    }
}
