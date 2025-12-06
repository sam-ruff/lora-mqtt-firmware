//! Integration test cases.

use colored::Colorize;

use crate::device::DeviceClient;
use crate::protocol::{CommandId, ResponseId, ResponseStatus};

/// Test result.
pub struct TestResult {
    pub name: String,
    pub passed: bool,
    pub message: Option<String>,
}

impl TestResult {
    fn pass(name: &str) -> Self {
        Self {
            name: name.to_string(),
            passed: true,
            message: None,
        }
    }

    fn fail(name: &str, message: &str) -> Self {
        Self {
            name: name.to_string(),
            passed: false,
            message: Some(message.to_string()),
        }
    }
}

/// Run a test function and print results as it happens.
fn run_test<F>(name: &str, device: &mut DeviceClient, test_fn: F) -> TestResult
where
    F: FnOnce(&mut DeviceClient) -> TestResult,
{
    print!("  {} ... ", name);
    std::io::Write::flush(&mut std::io::stdout()).ok();

    let mut result = test_fn(device);
    result.name = name.to_string();

    if result.passed {
        println!("{}", "PASS".green().bold());
    } else {
        println!("{}", "FAIL".red().bold());
        if let Some(msg) = &result.message {
            println!("    {}", msg.red());
        }
    }

    result
}

/// Run all tests and return results.
pub fn run_all_tests(device: &mut DeviceClient) -> Vec<TestResult> {
    let mut results = Vec::new();

    results.push(run_test("GetVersion returns version bytes", device, test_get_version));
    results.push(run_test("Invalid command returns error", device, test_invalid_command));
    results.push(run_test("Multiple GetVersion calls succeed", device, test_multiple_get_version));

    results
}

/// Print test results summary.
pub fn print_results(results: &[TestResult]) {
    println!("\n{}", "=".repeat(60));
    println!("{}", "Test Results".bold());
    println!("{}", "=".repeat(60));

    let mut passed = 0;
    let mut failed = 0;

    for result in results {
        if result.passed {
            println!("  {} {}", "[PASS]".green().bold(), result.name);
            passed += 1;
        } else {
            println!("  {} {}", "[FAIL]".red().bold(), result.name);
            if let Some(msg) = &result.message {
                println!("         {}", msg.red());
            }
            failed += 1;
        }
    }

    println!("{}", "-".repeat(60));
    println!(
        "  Total: {} passed, {} failed",
        passed.to_string().green(),
        if failed > 0 {
            failed.to_string().red()
        } else {
            failed.to_string().normal()
        }
    );
    println!("{}", "=".repeat(60));
}

// --- Individual Tests ---

fn test_get_version(device: &mut DeviceClient) -> TestResult {
    match device.send_command(CommandId::GetVersion, &[]) {
        Ok(response) => {
            if response.resp_id != ResponseId::Version {
                return TestResult::fail("test", &format!("Expected Version response, got {:?}", response.resp_id));
            }
            if response.payload.len() != 3 {
                return TestResult::fail(
                    "test",
                    &format!("Expected 3 bytes, got {}", response.payload.len()),
                );
            }

            let (major, minor, patch) = (
                response.payload[0],
                response.payload[1],
                response.payload[2],
            );
            print!("(v{}.{}.{}) ", major, minor, patch);
            TestResult::pass("test")
        }
        Err(e) => TestResult::fail("test", &format!("Error: {}", e)),
    }
}

fn test_invalid_command(device: &mut DeviceClient) -> TestResult {
    match device.send_raw_command(0xFE, &[]) {  // 0xFE is invalid, 0xFF is Error response ID
        Ok(response) => {
            // Should get an Error response with InvalidCommand status
            if response.resp_id != ResponseId::Error {
                return TestResult::fail(
                    "test",
                    &format!("Expected Error response, got {:?}", response.resp_id),
                );
            }
            // Error payload: [status, original_cmd_id]
            if response.payload.len() >= 1 {
                let status = response.payload[0];
                if status == ResponseStatus::InvalidCommand as u8 {
                    TestResult::pass("test")
                } else {
                    TestResult::fail(
                        "test",
                        &format!("Expected InvalidCommand status (0x01), got 0x{:02x}", status),
                    )
                }
            } else {
                TestResult::fail("test", "Error response payload too short")
            }
        }
        Err(e) => TestResult::fail("test", &format!("Error: {}", e)),
    }
}

fn test_multiple_get_version(device: &mut DeviceClient) -> TestResult {
    for i in 0..5 {
        match device.send_command(CommandId::GetVersion, &[]) {
            Ok(response) => {
                if response.resp_id != ResponseId::Version {
                    return TestResult::fail(
                        "test",
                        &format!("GetVersion {} failed: got {:?}", i + 1, response.resp_id),
                    );
                }
            }
            Err(e) => {
                return TestResult::fail("test", &format!("GetVersion {} error: {}", i + 1, e));
            }
        }
    }

    TestResult::pass("test")
}
