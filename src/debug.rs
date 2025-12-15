//! Debug logging via USB CDC.
//!
//! Provides macros for writing debug output to the secondary CDC-ACM port.
//! Output is non-blocking and will be dropped if the queue is full or
//! the debug port is not connected.

use core::fmt::Write;

use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::Channel;
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

/// Format and write a debug message.
///
/// This is the implementation behind the debug! macro.
pub fn debug_print(args: core::fmt::Arguments) {
    let mut s: String<MAX_DEBUG_MSG_LEN> = String::new();
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
