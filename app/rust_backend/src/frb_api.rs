//! Flutter Rust Bridge API.
//!
//! One process-wide hub client, selected at init: the real HTTP client, or
//! the in-memory fake for `--dart-define=MOCK=true`. Every exported function
//! returns `Result<T, String>`, which Dart sees as a throwing `Future`.

use once_cell::sync::Lazy;
use std::sync::Mutex;

use crate::client::{FakeHub, HttpHubApi, HubApi};
use crate::engine::{load_snapshot, save_config};
pub use crate::models::{ApplyOutcome, ConfigUpdate, HubConfigView, HubSnapshot, HubStatusView};

static API: Lazy<Mutex<Option<Box<dyn HubApi + Send>>>> = Lazy::new(|| Mutex::new(None));

fn with_api<F, R>(f: F) -> Result<R, String>
where
    F: FnOnce(&dyn HubApi) -> Result<R, String>,
{
    let guard = API.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let api = guard
        .as_ref()
        .ok_or_else(|| "Not initialised. Call frbInit first.".to_string())?;
    f(api.as_ref())
}

/// Initialise against the hub at `base_url` (`http://192.168.4.1` on the
/// provisioning hotspot), or against the in-memory fake in mock mode.
pub fn frb_init(base_url: String, mock: bool) -> Result<(), String> {
    let api: Box<dyn HubApi + Send> = if mock {
        Box::new(FakeHub::new())
    } else {
        Box::new(HttpHubApi::new(&base_url))
    };
    *API.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(api);
    Ok(())
}

/// Point the client at a different hub address (keeps mock mode as-is if the
/// fake is active - the fake ignores addresses).
pub fn frb_set_hub_address(base_url: String) -> Result<(), String> {
    let mut guard = API.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    match guard.as_ref() {
        Some(_) => {
            *guard = Some(Box::new(HttpHubApi::new(&base_url)));
            Ok(())
        }
        None => Err("Not initialised. Call frbInit first.".to_string()),
    }
}

/// Fetch the hub's configuration and live status in one go.
pub fn frb_load_snapshot() -> Result<HubSnapshot, String> {
    with_api(load_snapshot)
}

/// Fetch just the live status.
pub fn frb_get_status() -> Result<HubStatusView, String> {
    with_api(|api| api.fetch_status().map_err(|e| e.to_string()))
}

/// Validate and apply a configuration update; on success the hub reboots
/// onto the configured network.
pub fn frb_save_config(update: ConfigUpdate) -> Result<ApplyOutcome, String> {
    with_api(|api| save_config(api, update))
}
