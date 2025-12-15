//! LED task for non-blocking LED control
//!
//! Handles LED flashing operations without blocking other tasks.

use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::{Receiver, Sender};
use esp_hal::gpio::Output;

/// Duration of LED flash in milliseconds
const LED_FLASH_MS: u64 = 50;

/// LED flash duration configuration
#[derive(Clone, Copy)]
pub enum LedFlashDuration {
    /// Use the default flash duration
    Default,
    /// Use a custom flash duration in milliseconds
    #[allow(dead_code)]
    Ms(u64),
}

/// Type alias for the LED flash channel sender
pub type LedSender = Sender<'static, CriticalSectionRawMutex, LedFlashDuration, 4>;

/// Type alias for the LED flash channel receiver
pub type LedReceiver = Receiver<'static, CriticalSectionRawMutex, LedFlashDuration, 4>;

/// Channel for LED flash signals
pub static LED_CHANNEL: embassy_sync::channel::Channel<CriticalSectionRawMutex, LedFlashDuration, 4> =
    embassy_sync::channel::Channel::new();

/// Task that handles LED flashing without blocking other operations
pub async fn led_task(mut led: Output<'static>, receiver: LedReceiver) {
    loop {
        // Wait for flash signal
        let flash_duration = receiver.receive().await;
        let duration_ms = match flash_duration {
            LedFlashDuration::Default => LED_FLASH_MS,
            LedFlashDuration::Ms(ms) => ms,
        };

        // Flash LED (turn off then back on, since active low)
        led.set_high(); // LED off
        embassy_time::Timer::after(embassy_time::Duration::from_millis(duration_ms)).await;
        led.set_low(); // LED on
    }
}
