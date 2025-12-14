//! USB OTG module for dual CDC-ACM serial ports.
//!
//! Provides two virtual COM ports:
//! - CDC0: Data communication (commands/responses)
//! - CDC1: Debug log output

pub mod cdc_io;

pub use cdc_io::{CdcReader, CdcWriter};
