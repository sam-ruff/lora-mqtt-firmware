//! Admin task for system commands (reboot, etc.)
//!
//! Handles administrative commands that don't belong in other tasks.

use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::{Channel, Receiver};
#[cfg(feature = "embedded")]
use embassy_time::{Duration, Timer};

/// Admin command types
#[derive(Clone, Copy, Debug)]
pub enum AdminCommand {
    /// Normal reboot (restart firmware)
    Reboot,
}

/// Channel for admin commands
pub static ADMIN_CHANNEL: Channel<CriticalSectionRawMutex, AdminCommand, 4> = Channel::new();

/// Type alias for the admin command receiver
pub type AdminReceiver = Receiver<'static, CriticalSectionRawMutex, AdminCommand, 4>;

/// Reboot the device (normal restart)
#[cfg(feature = "embedded")]
fn reboot() -> ! {
    esp_hal::system::software_reset()
}

/// Admin task that handles system commands
///
/// This task listens for admin commands on the ADMIN_CHANNEL and executes them.
#[cfg(feature = "embedded")]
pub async fn admin_task(receiver: AdminReceiver) {
    loop {
        let cmd = receiver.receive().await;

        match cmd {
            AdminCommand::Reboot => {
                crate::debug!("Rebooting...");
                // Allow the debug message to send
                Timer::after(Duration::from_millis(500)).await;
                reboot();
            }
        }
    }
}

/// Admin task stub for non-embedded builds (tests)
#[cfg(not(feature = "embedded"))]
pub async fn admin_task(_receiver: AdminReceiver) {
    // In tests, just wait forever
    loop {
        embassy_futures::yield_now().await;
    }
}
