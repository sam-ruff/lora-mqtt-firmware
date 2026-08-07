//! Orchestration over the hub API: validation before writes, combined
//! reads. Pure over the [`HubApi`] trait so everything unit-tests against
//! the mocked hub.

use hub_protocol::{MAX_CLIENT_ID_LEN, MAX_HOST_LEN, MAX_PASSWORD_LEN, MAX_SSID_LEN};

use crate::client::HubApi;
use crate::models::{ApplyOutcome, ConfigUpdate, HubSnapshot};

/// Validate an update against the same limits the firmware enforces, so bad
/// values fail in the app with a readable message instead of a 400.
pub fn validate_update(update: &ConfigUpdate) -> Result<(), String> {
    check_len(update.wifi_ssid.as_deref(), MAX_SSID_LEN, "WiFi name")?;
    check_len(update.wifi_password.as_deref(), MAX_PASSWORD_LEN, "WiFi password")?;
    check_len(update.mqtt_host.as_deref(), MAX_HOST_LEN, "broker host")?;
    check_len(update.client_id.as_deref(), MAX_CLIENT_ID_LEN, "client id")?;
    check_len(update.gw_host.as_deref(), MAX_HOST_LEN, "network server host")?;

    if update.mqtt_port == Some(0) || update.gw_port == Some(0) {
        return Err("port cannot be 0".into());
    }
    if let Some(mode) = update.mode.as_deref() {
        if mode != "bridge" && mode != "gateway" {
            return Err("mode must be bridge or gateway".into());
        }
    }
    if let Some(sf) = update.gw_sf {
        if !(7..=12).contains(&sf) {
            return Err("spreading factor must be 7 to 12".into());
        }
    }
    if let Some(bw) = update.gw_bw_khz {
        if !matches!(bw, 125 | 250 | 500) {
            return Err("bandwidth must be 125, 250 or 500 kHz".into());
        }
    }
    if let Some(freq) = update.gw_freq_hz {
        if !(150_000_000..=960_000_000).contains(&freq) {
            return Err("frequency out of the radio's range".into());
        }
    }
    Ok(())
}

fn check_len(value: Option<&str>, max: usize, what: &str) -> Result<(), String> {
    match value {
        Some(v) if v.len() > max => Err(format!("{what} is longer than {max} bytes")),
        _ => Ok(()),
    }
}

/// One round trip for the connect screen: config plus live status.
pub fn load_snapshot(api: &dyn HubApi) -> Result<HubSnapshot, String> {
    let config = api.fetch_config().map_err(|e| e.to_string())?;
    let status = api.fetch_status().map_err(|e| e.to_string())?;
    Ok(HubSnapshot { config, status })
}

/// Validate, then apply. The hub reboots itself on success.
pub fn save_config(api: &dyn HubApi, update: ConfigUpdate) -> Result<ApplyOutcome, String> {
    validate_update(&update)?;
    api.apply_config(update).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::{ApiError, MockHubApi};
    use crate::models::{HubConfigView, HubStatusView};
    use mockall::predicate::eq;

    fn config() -> HubConfigView {
        HubConfigView {
            mode: "bridge".into(),
            wifi_ssid: "Net".into(),
            mqtt_host: "10.0.0.2".into(),
            mqtt_port: 1883,
            client_id: String::new(),
            gw_freq_hz: 868_100_000,
            gw_sf: 7,
            gw_bw_khz: 125,
            gw_cr: 5,
            gw_host: String::new(),
            gw_port: 1700,
        }
    }

    fn status() -> HubStatusView {
        HubStatusView {
            wifi_state: 2,
            mqtt_state: 2,
            ip: "192.168.1.9".into(),
            uplink: 1,
            downlink: 0,
            dropped: 0,
            uptime_secs: 60,
        }
    }

    #[test]
    fn load_snapshot_combines_both_calls() {
        let mut api = MockHubApi::new();
        api.expect_fetch_config().times(1).returning(|| Ok(config()));
        api.expect_fetch_status().times(1).returning(|| Ok(status()));
        let snapshot = load_snapshot(&api).unwrap();
        assert_eq!(snapshot.config, config());
        assert_eq!(snapshot.status, status());
    }

    #[test]
    fn load_snapshot_surfaces_unreachable() {
        let mut api = MockHubApi::new();
        api.expect_fetch_config()
            .returning(|| Err(ApiError::Unreachable("timeout".into())));
        let err = load_snapshot(&api).unwrap_err();
        assert!(err.contains("unreachable"));
    }

    #[test]
    fn save_sends_the_validated_update() {
        let update = ConfigUpdate {
            wifi_ssid: Some("Net".into()),
            wifi_password: Some("pw".into()),
            mode: Some("gateway".into()),
            ..ConfigUpdate::default()
        };
        let mut api = MockHubApi::new();
        api.expect_apply_config()
            .with(eq(update.clone()))
            .times(1)
            .returning(|_| Ok(ApplyOutcome { ok: true, rebooting: true }));
        let outcome = save_config(&api, update).unwrap();
        assert!(outcome.ok && outcome.rebooting);
    }

    #[test]
    fn invalid_updates_never_reach_the_network() {
        let mut api = MockHubApi::new();
        api.expect_apply_config().times(0);

        for update in [
            ConfigUpdate { gw_sf: Some(6), ..Default::default() },
            ConfigUpdate { gw_bw_khz: Some(100), ..Default::default() },
            ConfigUpdate { mqtt_port: Some(0), ..Default::default() },
            ConfigUpdate { mode: Some("nonsense".into()), ..Default::default() },
            ConfigUpdate { wifi_ssid: Some("x".repeat(33)), ..Default::default() },
        ] {
            assert!(save_config(&api, update).is_err());
        }
    }

    #[test]
    fn rejected_apply_surfaces_the_reason() {
        let mut api = MockHubApi::new();
        api.expect_apply_config()
            .returning(|_| Err(ApiError::Rejected("HTTP 422".into())));
        let err = save_config(
            &api,
            ConfigUpdate { wifi_ssid: Some("Net".into()), ..Default::default() },
        )
        .unwrap_err();
        assert!(err.contains("422"));
    }
}
