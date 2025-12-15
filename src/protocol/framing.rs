//! Frame accumulator for COBS-encoded serial protocol
//!
//! Accumulates bytes until a complete frame (delimited by 0x00) is received.

use crate::config::protocol::{FRAME_DELIMITER, MAX_FRAME_SIZE};
use heapless::Vec;

/// Accumulates incoming bytes and extracts complete COBS frames.
///
/// Frames are delimited by zero bytes (0x00). The accumulator buffers
/// non-zero bytes until a delimiter is received, then returns the
/// complete frame for COBS decoding.
pub struct FrameAccumulator {
    buffer: Vec<u8, MAX_FRAME_SIZE>,
}

impl FrameAccumulator {
    /// Create a new empty frame accumulator.
    pub fn new() -> Self {
        Self {
            buffer: Vec::new(),
        }
    }

    /// Push a byte into the accumulator.
    ///
    /// Returns `Some(frame)` when a complete frame is detected (delimiter received).
    /// Returns `None` if more bytes are needed or if the frame was empty.
    pub fn push(&mut self, byte: u8) -> Option<Vec<u8, MAX_FRAME_SIZE>> {
        if byte == FRAME_DELIMITER {
            if self.buffer.is_empty() {
                // Empty frame or leading delimiter, ignore
                return None;
            }

            // Frame complete - swap out the buffer
            let frame = core::mem::replace(&mut self.buffer, Vec::new());
            return Some(frame);
        }

        // Non-delimiter byte
        if self.buffer.push(byte).is_err() {
            // Buffer overflow - reset and drop this frame
            self.buffer.clear();
            return None;
        }

        None
    }

    /// Reset the accumulator, discarding any partial frame.
    #[allow(dead_code)]
    pub fn reset(&mut self) {
        self.buffer.clear();
    }

    /// Returns true if the buffer is empty (no partial frame in progress).
    #[allow(dead_code)]
    pub fn is_empty(&self) -> bool {
        self.buffer.is_empty()
    }

    /// Returns the current number of bytes in the buffer.
    #[allow(dead_code)]
    pub fn len(&self) -> usize {
        self.buffer.len()
    }
}

impl Default for FrameAccumulator {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_single_frame() {
        let mut acc = FrameAccumulator::new();

        // Push some bytes
        assert!(acc.push(0x01).is_none());
        assert!(acc.push(0x02).is_none());
        assert!(acc.push(0x03).is_none());

        // Push delimiter
        let frame = acc.push(0x00).expect("Should return frame");
        assert_eq!(frame.as_slice(), &[0x01, 0x02, 0x03]);

        // Accumulator should be empty now
        assert!(acc.is_empty());
    }

    #[test]
    fn test_empty_frame_ignored() {
        let mut acc = FrameAccumulator::new();

        // Leading delimiter should be ignored
        assert!(acc.push(0x00).is_none());
        assert!(acc.is_empty());

        // Multiple delimiters should be ignored
        assert!(acc.push(0x00).is_none());
        assert!(acc.push(0x00).is_none());
    }

    #[test]
    fn test_multiple_frames() {
        let mut acc = FrameAccumulator::new();

        // First frame
        acc.push(0x01);
        acc.push(0x02);
        let frame1 = acc.push(0x00).expect("Should return frame");
        assert_eq!(frame1.as_slice(), &[0x01, 0x02]);

        // Second frame
        acc.push(0x03);
        acc.push(0x04);
        acc.push(0x05);
        let frame2 = acc.push(0x00).expect("Should return frame");
        assert_eq!(frame2.as_slice(), &[0x03, 0x04, 0x05]);
    }

    #[test]
    fn test_reset() {
        let mut acc = FrameAccumulator::new();

        acc.push(0x01);
        acc.push(0x02);
        assert!(!acc.is_empty());

        acc.reset();
        assert!(acc.is_empty());

        // Should be able to receive new frame
        acc.push(0x03);
        let frame = acc.push(0x00).expect("Should return frame");
        assert_eq!(frame.as_slice(), &[0x03]);
    }
}
