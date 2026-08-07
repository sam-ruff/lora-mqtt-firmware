//! HTTP request parsing, routing and payloads for the provisioning portal.
//!
//! One tiny hand-rolled HTTP/1.1 server surface: the embedded config page,
//! a JSON API a phone app can drive (`/api/config`, `/api/status`), and a
//! captive-portal redirect for every other path so OS connectivity checks
//! land on the page.

use heapless::{String, Vec};
use hub_protocol::{
    GatewaySettings, HubCommand, HubMode, HubStatus, MqttSettings, WifiSettings,
};

use crate::hub_config::HubConfig;

/// Largest request we accept (headers + JSON body).
pub const MAX_REQUEST: usize = 2048;
/// Response body scratch size for the JSON endpoints.
pub const MAX_BODY: usize = 512;

#[derive(Debug, PartialEq, Eq)]
pub struct HttpRequest<'a> {
    pub method: &'a str,
    pub path: &'a str,
    pub body: &'a [u8],
}

#[derive(Debug, PartialEq, Eq)]
pub enum ParseOutcome<'a> {
    /// A full request; the usize is the bytes consumed.
    Complete(HttpRequest<'a>),
    /// Headers or body still incomplete; read more bytes.
    Incomplete,
    /// Not parseable as HTTP; drop the connection.
    Bad,
}

/// Try to parse one complete request from the buffered bytes.
pub fn try_parse(buf: &[u8]) -> ParseOutcome<'_> {
    let Some(headers_end) = find_headers_end(buf) else {
        return if buf.len() >= MAX_REQUEST { ParseOutcome::Bad } else { ParseOutcome::Incomplete };
    };
    let Ok(head) = core::str::from_utf8(&buf[..headers_end]) else {
        return ParseOutcome::Bad;
    };
    let mut lines = head.split("\r\n");
    let Some(request_line) = lines.next() else {
        return ParseOutcome::Bad;
    };
    let mut parts = request_line.split(' ');
    let (Some(method), Some(path)) = (parts.next(), parts.next()) else {
        return ParseOutcome::Bad;
    };

    let mut content_length = 0usize;
    for line in lines {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        if name.eq_ignore_ascii_case("content-length") {
            content_length = value.trim().parse().unwrap_or(0);
        }
    }
    if content_length > MAX_REQUEST - headers_end {
        return ParseOutcome::Bad;
    }
    let body_end = headers_end + 4 + content_length;
    if buf.len() < body_end {
        return ParseOutcome::Incomplete;
    }
    ParseOutcome::Complete(HttpRequest {
        method,
        path,
        body: &buf[headers_end + 4..body_end],
    })
}

fn find_headers_end(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|w| w == b"\r\n\r\n")
}

#[derive(Debug, PartialEq, Eq)]
pub enum Route<'a> {
    Index,
    ApiConfigGet,
    ApiConfigPost(&'a [u8]),
    ApiStatus,
    /// Everything else: captive-portal redirect to the index page.
    Redirect,
}

pub fn route<'a>(request: &HttpRequest<'a>) -> Route<'a> {
    match (request.method, request.path) {
        ("GET", "/") | ("GET", "/index.html") => Route::Index,
        ("GET", "/api/config") => Route::ApiConfigGet,
        ("POST", "/api/config") => Route::ApiConfigPost(request.body),
        ("GET", "/api/status") => Route::ApiStatus,
        _ => Route::Redirect,
    }
}

/// The `/api/config` GET payload (no password).
#[derive(serde::Serialize)]
struct ConfigView<'a> {
    mode: &'a str,
    wifi_ssid: &'a str,
    mqtt_host: &'a str,
    mqtt_port: u16,
    client_id: &'a str,
    gw_freq_hz: u32,
    gw_sf: u8,
    gw_bw_khz: u32,
    gw_cr: u8,
    gw_host: &'a str,
    gw_port: u16,
}

pub fn config_json(config: &HubConfig) -> Vec<u8, MAX_BODY> {
    let view = ConfigView {
        mode: match config.mode {
            HubMode::Bridge => "bridge",
            HubMode::LorawanGateway => "gateway",
        },
        wifi_ssid: &config.wifi.ssid,
        mqtt_host: &config.mqtt.host,
        mqtt_port: config.mqtt.port,
        client_id: &config.mqtt.client_id,
        gw_freq_hz: config.gateway.frequency_hz,
        gw_sf: config.gateway.spreading_factor,
        gw_bw_khz: config.gateway.bandwidth_khz,
        gw_cr: config.gateway.coding_rate,
        gw_host: &config.gateway.host,
        gw_port: config.gateway.port,
    };
    to_json(&view)
}

/// The `/api/status` GET payload.
#[derive(serde::Serialize)]
struct StatusView<'a> {
    wifi_state: u8,
    mqtt_state: u8,
    ip: &'a str,
    uplink: u32,
    downlink: u32,
    dropped: u32,
    uptime_secs: u32,
}

pub fn status_json(status: &HubStatus) -> Vec<u8, MAX_BODY> {
    let mut ip: String<15> = String::new();
    let _ = core::fmt::Write::write_fmt(
        &mut ip,
        format_args!("{}.{}.{}.{}", status.ip[0], status.ip[1], status.ip[2], status.ip[3]),
    );
    let view = StatusView {
        wifi_state: status.wifi_state as u8,
        mqtt_state: status.mqtt_state as u8,
        ip: &ip,
        uplink: status.uplink_count,
        downlink: status.downlink_count,
        dropped: status.dropped_count,
        uptime_secs: status.uptime_secs,
    };
    to_json(&view)
}

fn to_json<T: serde::Serialize>(value: &T) -> Vec<u8, MAX_BODY> {
    let mut buf = [0u8; MAX_BODY];
    let len = serde_json_core::to_slice(value, &mut buf).unwrap_or(0);
    let mut out = Vec::new();
    let _ = out.extend_from_slice(&buf[..len]);
    out
}

/// The `/api/config` POST body: every field optional so the page (which
/// posts everything) and an app (which may post one group) both work.
#[derive(serde::Deserialize)]
struct ConfigUpdate<'a> {
    wifi_ssid: Option<&'a str>,
    #[serde(default)]
    wifi_password: Option<&'a str>,
    mqtt_host: Option<&'a str>,
    mqtt_port: Option<u16>,
    #[serde(default)]
    client_id: Option<&'a str>,
    mode: Option<&'a str>,
    gw_freq_hz: Option<u32>,
    gw_sf: Option<u8>,
    gw_bw_khz: Option<u32>,
    gw_cr: Option<u8>,
    gw_host: Option<&'a str>,
    gw_port: Option<u16>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpdateError {
    /// Body is not valid JSON or a value does not fit its field.
    Bad,
    /// Valid JSON but nothing recognisable to change.
    Empty,
}

/// Turn a POST body into the hub commands to apply, in order.
pub fn commands_from_update(
    body: &[u8],
    current: &HubConfig,
) -> Result<Vec<HubCommand, 4>, UpdateError> {
    let (update, _) =
        serde_json_core::from_slice::<ConfigUpdate>(body).map_err(|_| UpdateError::Bad)?;
    let mut commands: Vec<HubCommand, 4> = Vec::new();

    if let Some(ssid) = update.wifi_ssid {
        let wifi = WifiSettings {
            ssid: String::try_from(ssid).map_err(|_| UpdateError::Bad)?,
            password: String::try_from(update.wifi_password.unwrap_or(""))
                .map_err(|_| UpdateError::Bad)?,
        };
        let _ = commands.push(HubCommand::SetWifiConfig(wifi));
    }
    if update.mqtt_host.is_some() || update.mqtt_port.is_some() || update.client_id.is_some() {
        let mqtt = MqttSettings {
            host: match update.mqtt_host {
                Some(host) => String::try_from(host).map_err(|_| UpdateError::Bad)?,
                None => current.mqtt.host.clone(),
            },
            port: update.mqtt_port.unwrap_or(current.mqtt.port),
            client_id: match update.client_id {
                Some(id) => String::try_from(id).map_err(|_| UpdateError::Bad)?,
                None => current.mqtt.client_id.clone(),
            },
        };
        let _ = commands.push(HubCommand::SetMqttConfig(mqtt));
    }
    let gateway_touched = update.gw_freq_hz.is_some()
        || update.gw_sf.is_some()
        || update.gw_bw_khz.is_some()
        || update.gw_cr.is_some()
        || update.gw_host.is_some()
        || update.gw_port.is_some();
    if gateway_touched {
        let gateway = GatewaySettings {
            frequency_hz: update.gw_freq_hz.unwrap_or(current.gateway.frequency_hz),
            spreading_factor: update.gw_sf.unwrap_or(current.gateway.spreading_factor),
            bandwidth_khz: update.gw_bw_khz.unwrap_or(current.gateway.bandwidth_khz),
            coding_rate: update.gw_cr.unwrap_or(current.gateway.coding_rate),
            host: match update.gw_host {
                Some(host) => String::try_from(host).map_err(|_| UpdateError::Bad)?,
                None => current.gateway.host.clone(),
            },
            port: update.gw_port.unwrap_or(current.gateway.port),
        };
        let _ = commands.push(HubCommand::SetGatewayConfig(gateway));
    }
    if let Some(mode) = update.mode {
        let mode = match mode {
            "bridge" => HubMode::Bridge,
            "gateway" => HubMode::LorawanGateway,
            _ => return Err(UpdateError::Bad),
        };
        let _ = commands.push(HubCommand::SetMode { mode });
    }

    if commands.is_empty() {
        return Err(UpdateError::Empty);
    }
    Ok(commands)
}

/// The embedded configuration page. Prefills from `/api/config`, posts JSON
/// back, and tells the user the hub restarts to apply.
pub const INDEX_HTML: &str = r#"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1">
<title>Walkie-Textie Hub</title>
<style>
body{font-family:system-ui,sans-serif;margin:0;background:#f2f2f0;color:#1c1c1c}
main{max-width:26rem;margin:0 auto;padding:1.2rem}
h1{font-size:1.3rem}
fieldset{border:1px solid #ccc;border-radius:8px;margin:0 0 1rem;padding:.8rem}
legend{font-weight:600;padding:0 .3rem}
label{display:block;font-size:.85rem;margin:.5rem 0 .15rem}
input,select{width:100%;box-sizing:border-box;padding:.5rem;border:1px solid #bbb;border-radius:6px;font-size:1rem}
button{width:100%;padding:.7rem;border:0;border-radius:8px;background:#128c7e;color:#fff;font-size:1rem;font-weight:600}
#msg{margin-top:.8rem;font-size:.9rem;min-height:1.2rem}
.gw{display:none}
</style>
</head>
<body>
<main>
<h1>Walkie-Textie Hub setup</h1>
<form id="f">
<fieldset><legend>WiFi</legend>
<label>Network name (SSID)</label><input name="wifi_ssid" required>
<label>Password</label><input name="wifi_password" type="password" placeholder="leave blank to keep">
</fieldset>
<fieldset><legend>Mode</legend>
<select name="mode" id="mode">
<option value="bridge">MQTT bridge</option>
<option value="gateway">LoRaWAN gateway</option>
</select>
</fieldset>
<fieldset><legend>MQTT broker</legend>
<label>Host</label><input name="mqtt_host">
<label>Port</label><input name="mqtt_port" type="number" value="1883">
</fieldset>
<fieldset class="gw" id="gwbox"><legend>LoRaWAN network server</legend>
<label>Host</label><input name="gw_host">
<label>Port</label><input name="gw_port" type="number" value="1700">
<label>Frequency (Hz)</label><input name="gw_freq_hz" type="number" value="868100000">
<label>Spreading factor</label><input name="gw_sf" type="number" min="7" max="12" value="7">
</fieldset>
<button type="submit">Save and restart</button>
<div id="msg"></div>
</form>
<script>
const f=document.getElementById('f'),m=document.getElementById('msg');
const showGw=()=>{document.getElementById('gwbox').style.display=f.mode.value==='gateway'?'block':'none'};
f.mode.addEventListener('change',showGw);
fetch('/api/config').then(r=>r.json()).then(c=>{
 f.wifi_ssid.value=c.wifi_ssid;f.mode.value=c.mode;f.mqtt_host.value=c.mqtt_host;
 f.mqtt_port.value=c.mqtt_port;f.gw_host.value=c.gw_host;f.gw_port.value=c.gw_port;
 f.gw_freq_hz.value=c.gw_freq_hz;f.gw_sf.value=c.gw_sf;showGw();
}).catch(()=>{});
f.addEventListener('submit',e=>{
 e.preventDefault();
 const d={wifi_ssid:f.wifi_ssid.value,mode:f.mode.value};
 if(f.wifi_password.value)d.wifi_password=f.wifi_password.value;
 if(f.mqtt_host.value){d.mqtt_host=f.mqtt_host.value;d.mqtt_port=+f.mqtt_port.value}
 if(f.mode.value==='gateway'&&f.gw_host.value){d.gw_host=f.gw_host.value;d.gw_port=+f.gw_port.value;
  d.gw_freq_hz=+f.gw_freq_hz.value;d.gw_sf=+f.gw_sf.value}
 m.textContent='Saving...';
 fetch('/api/config',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify(d)})
 .then(r=>r.json()).then(j=>{m.textContent=j.ok?'Saved. The hub is restarting and will join your WiFi.':'Rejected: '+(j.error||'check the values')})
 .catch(()=>{m.textContent='No reply - the hub may already be restarting.'});
});
</script>
</main>
</body>
</html>
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_get_and_routes_it() {
        let raw = b"GET /api/status HTTP/1.1\r\nHost: x\r\n\r\n";
        let ParseOutcome::Complete(request) = try_parse(raw) else {
            panic!("should parse");
        };
        assert_eq!(route(&request), Route::ApiStatus);
    }

    #[test]
    fn waits_for_the_full_post_body() {
        let raw = b"POST /api/config HTTP/1.1\r\nContent-Length: 10\r\n\r\n12345";
        assert_eq!(try_parse(raw), ParseOutcome::Incomplete);

        let raw = b"POST /api/config HTTP/1.1\r\nContent-Length: 5\r\n\r\n12345";
        let ParseOutcome::Complete(request) = try_parse(raw) else {
            panic!("should parse");
        };
        assert_eq!(request.body, b"12345");
        assert!(matches!(route(&request), Route::ApiConfigPost(b"12345")));
    }

    #[test]
    fn unknown_paths_redirect_for_the_captive_portal() {
        let raw = b"GET /generate_204 HTTP/1.1\r\n\r\n";
        let ParseOutcome::Complete(request) = try_parse(raw) else {
            panic!("should parse");
        };
        assert_eq!(route(&request), Route::Redirect);
    }

    #[test]
    fn config_json_never_contains_the_password() {
        let mut config = HubConfig::default();
        config.wifi.password = String::try_from("secret").unwrap();
        let json = config_json(&config);
        assert!(!json.windows(6).any(|w| w == b"secret"));
    }

    #[test]
    fn update_builds_commands_in_order() {
        let body = br#"{"wifi_ssid":"Net","wifi_password":"pw","mqtt_host":"10.0.0.2","mqtt_port":1883,"mode":"gateway","gw_host":"10.0.0.3"}"#;
        let commands = commands_from_update(body, &HubConfig::default()).unwrap();
        assert_eq!(commands.len(), 4);
        assert!(matches!(commands[0], HubCommand::SetWifiConfig(_)));
        assert!(matches!(commands[1], HubCommand::SetMqttConfig(_)));
        assert!(matches!(commands[2], HubCommand::SetGatewayConfig(_)));
        assert!(matches!(commands[3], HubCommand::SetMode { mode: HubMode::LorawanGateway }));
    }

    #[test]
    fn partial_update_keeps_current_values() {
        let mut current = HubConfig::default();
        current.mqtt.host = String::try_from("old-host").unwrap();
        let body = br#"{"mqtt_port":1884}"#;
        let commands = commands_from_update(body, &current).unwrap();
        let HubCommand::SetMqttConfig(mqtt) = &commands[0] else {
            panic!("expected SetMqttConfig");
        };
        assert_eq!(mqtt.host.as_str(), "old-host");
        assert_eq!(mqtt.port, 1884);
    }

    #[test]
    fn bad_and_empty_updates_are_rejected() {
        let current = HubConfig::default();
        assert_eq!(
            commands_from_update(b"not json", &current),
            Err(UpdateError::Bad)
        );
        assert_eq!(commands_from_update(b"{}", &current), Err(UpdateError::Empty));
        assert_eq!(
            commands_from_update(br#"{"mode":"nonsense"}"#, &current),
            Err(UpdateError::Bad)
        );
    }
}
