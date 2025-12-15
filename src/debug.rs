//! Debug logging via USB CDC.
//!
//! Provides macros for writing debug output to the secondary CDC-ACM port.
//! Output is non-blocking and will be dropped if the queue is full or
//! the debug port is not connected.

use core::fmt::Write;

use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::Channel;
use embassy_time::Instant;
use embassy_usb::class::cdc_acm::Sender;
use esp_hal::otg_fs::asynch::Driver;
use heapless::String;

/// Maximum length of a single debug message
const MAX_DEBUG_MSG_LEN: usize = 128;

/// Maximum number of queued debug messages
const DEBUG_QUEUE_SIZE: usize = 16;

/// Channel for debug messages (proper queue instead of single buffer)
static DEBUG_CHANNEL: Channel<CriticalSectionRawMutex, String<MAX_DEBUG_MSG_LEN>, DEBUG_QUEUE_SIZE> =
    Channel::new();

/// Debug writer task that sends queued messages to the CDC port.
///
/// This task should be spawned and will continuously send debug messages
/// to the CDC sender when they become available.
pub async fn debug_writer_task(mut sender: Sender<'static, Driver<'static>>) {
    let receiver = DEBUG_CHANNEL.receiver();

    loop {
        // Wait for a debug message
        let msg = receiver.receive().await;

        // Try to send, ignore errors (port might not be connected)
        let _ = sender.write_packet(msg.as_bytes()).await;
        // Send newline
        let _ = sender.write_packet(b"\r\n").await;
    }
}

/// Format and write a debug message with timestamp.
///
/// This is the implementation behind the debug! macro.
/// Messages are prefixed with a timestamp in [MM:SS.mmm] format.
pub fn debug_print(args: core::fmt::Arguments) {
    let mut s: String<MAX_DEBUG_MSG_LEN> = String::new();

    // Get time since boot
    let now = Instant::now();
    let total_ms = now.as_millis();
    let total_secs = total_ms / 1000;
    let mins = total_secs / 60;
    let secs = total_secs % 60;
    let ms = total_ms % 1000;

    // Format timestamp as [MM:SS.mmm]
    let _ = write!(s, "[{:02}:{:02}.{:03}] ", mins, secs, ms);
    let _ = s.write_fmt(args);
    let _ = DEBUG_CHANNEL.try_send(s);
}

/// Print a debug message to the debug CDC port.
///
/// Usage: `debug!("Hello, {}!", "world");`
///
/// Messages are non-blocking and will be dropped if the debug port
/// is not connected or the queue is full.
#[macro_export]
macro_rules! debug {
    ($($arg:tt)*) => {
        $crate::debug::debug_print(format_args!($($arg)*))
    };
}
