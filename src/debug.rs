//! Debug logging via USB CDC.
//!
//! Provides macros for writing debug output to the secondary CDC-ACM port.
//! Output is non-blocking and will be dropped if the buffer is full or
//! the debug port is not connected.

use core::cell::RefCell;
use core::fmt::Write;

use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::blocking_mutex::Mutex;
use embassy_sync::signal::Signal;
use embassy_usb::class::cdc_acm::Sender;
use esp_hal::otg_fs::asynch::Driver;
use heapless::String;

/// Maximum length of a single debug message
const MAX_DEBUG_MSG_LEN: usize = 256;

/// Signal to indicate debug output is available
pub static DEBUG_SIGNAL: Signal<CriticalSectionRawMutex, ()> = Signal::new();

/// Buffer for pending debug messages (protected by critical section mutex)
static DEBUG_BUFFER: Mutex<CriticalSectionRawMutex, RefCell<Option<String<MAX_DEBUG_MSG_LEN>>>> =
    Mutex::new(RefCell::new(None));

/// Initialise the debug output system.
///
/// Must be called once during startup before using debug macros.
pub fn init() {
    DEBUG_BUFFER.lock(|cell| {
        cell.replace(Some(String::new()));
    });
}

/// Check if debug output is initialised.
pub fn is_init() -> bool {
    DEBUG_BUFFER.lock(|cell| cell.borrow().is_some())
}

/// Write a debug message to the buffer.
///
/// This is non-blocking and will truncate if the message is too long.
/// Returns true if the message was queued, false if debug is not initialised.
pub fn write_debug(msg: &str) -> bool {
    DEBUG_BUFFER.lock(|cell| {
        let mut borrowed = cell.borrow_mut();
        if let Some(ref mut buffer) = *borrowed {
            // Clear and write new message (we only keep the latest)
            buffer.clear();
            let _ = buffer.push_str(msg);
            DEBUG_SIGNAL.signal(());
            true
        } else {
            false
        }
    })
}

/// Take the current debug message from the buffer.
///
/// Returns None if no message is available.
pub fn take_debug_message() -> Option<String<MAX_DEBUG_MSG_LEN>> {
    DEBUG_BUFFER.lock(|cell| {
        let mut borrowed = cell.borrow_mut();
        if let Some(ref mut buffer) = *borrowed {
            if buffer.is_empty() {
                None
            } else {
                let msg = buffer.clone();
                buffer.clear();
                Some(msg)
            }
        } else {
            None
        }
    })
}

/// Debug writer task that sends buffered messages to the CDC port.
///
/// This task should be spawned and will continuously send debug messages
/// to the CDC sender when they become available.
pub async fn debug_writer_task(mut sender: Sender<'static, Driver<'static>>) {
    loop {
        // Wait for a debug message
        DEBUG_SIGNAL.wait().await;

        // Get the message
        if let Some(msg) = take_debug_message() {
            // Try to send, ignore errors (port might not be connected)
            let _ = sender.write_packet(msg.as_bytes()).await;
            // Send newline
            let _ = sender.write_packet(b"\r\n").await;
        }
    }
}

/// Format and write a debug message.
///
/// This is the implementation behind the debug! macro.
pub fn debug_print(args: core::fmt::Arguments) {
    let mut s: String<MAX_DEBUG_MSG_LEN> = String::new();
    let _ = s.write_fmt(args);
    write_debug(&s);
}

/// Print a debug message to the debug CDC port.
///
/// Usage: `debug!("Hello, {}!", "world");`
///
/// Messages are non-blocking and will be dropped if the debug port
/// is not connected or the buffer is full.
#[macro_export]
macro_rules! debug {
    ($($arg:tt)*) => {
        $crate::debug::debug_print(format_args!($($arg)*))
    };
}
