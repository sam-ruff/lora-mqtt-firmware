pub mod airtime;
pub mod calibration;
pub mod duty_cycle;
#[cfg(any(feature = "embedded", feature = "host-test"))]
pub mod driver;
#[cfg(any(feature = "embedded", feature = "host-test"))]
pub mod traits;
