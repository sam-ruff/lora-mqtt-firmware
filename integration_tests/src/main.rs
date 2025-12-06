//! Integration tests for walkie-textie firmware.
//!
//! Run after flashing the firmware to test basic serial communication.

mod device;
mod protocol;
mod tests;

use clap::Parser;
use colored::Colorize;

use device::DeviceClient;
use tests::{print_results, run_all_tests};

#[derive(Parser)]
#[command(name = "integration-tests")]
#[command(about = "Integration tests for walkie-textie firmware")]
struct Args {
    /// Serial port for the device
    #[arg(short, long, default_value = "/dev/ttyACM0")]
    port: String,

    /// Baud rate
    #[arg(short, long, default_value = "115200")]
    baud: u32,
}

fn main() -> anyhow::Result<()> {
    let args = Args::parse();

    println!("{}", "Walkie-Textie Integration Tests".bold());
    println!("Port: {}", args.port);
    println!("Baud: {}", args.baud);
    println!();

    println!("Connecting to device...");
    let mut device = DeviceClient::new(&args.port, args.baud)?;

    // Wait for bootloader output to finish, then clear buffer
    std::thread::sleep(std::time::Duration::from_secs(1));
    device.clear_buffer()?;
    println!("{}", "Connected!".green());

    println!("\nRunning tests...\n");

    let results = run_all_tests(&mut device);
    print_results(&results);

    // Exit with error code if any tests failed
    let failed = results.iter().filter(|r| !r.passed).count();
    if failed > 0 {
        std::process::exit(1);
    }

    Ok(())
}
