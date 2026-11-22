use serde_json::json;
use std::path::Path;

use super::adb::run_adb_cmd;
use super::remember;
use super::touch::adb;
use super::AndroidParams;
use std::time::Duration;

/// `adb devices -l`, never longer than 5 s. When nothing is ready, the remembered Wi-Fi device is looked for again (its
/// saved address, then the rest of its subnet), since a TV can come back under a new address.
pub fn handle_devices(_workspace_path: Option<&Path>) -> String {
    let list = || adb(None, &["devices", "-l"], Duration::from_secs(5));
    let mut out = match list() {
        Ok(o) => o,
        Err(e) => return json!({ "success": false, "action": "devices", "error": format!("{}. If adb is stuck: mysid android restart_server", e) }).to_string(),
    };
    let mut note = None;
    if super::touch::list_devices().map_or(false, |(ready, _)| ready.is_empty()) {
        note = remember::reconnect();
        if let Ok(o) = list() {
            out = o;
        }
    }
    json!({
        "success": out.code == 0,
        "action": "devices",
        "exit_code": out.code,
        "stdout": String::from_utf8_lossy(&out.stdout).trim(),
        "stderr": out.stderr.trim(),
        "remembered_device": note,
    }).to_string()
}

pub fn handle_disconnect(workspace_path: Option<&Path>) -> String {
    match run_adb_cmd(&["disconnect"], workspace_path) {
        Ok((code, stdout, stderr)) => json!({
            "success": code == 0,
            "action": "disconnect",
            "stdout": stdout.trim(),
            "stderr": stderr.trim(),
        }).to_string(),
        Err(e) => json!({ "success": false, "error": e }).to_string(),
    }
}

pub fn handle_restart_server(workspace_path: Option<&Path>) -> String {
    let _ = run_adb_cmd(&["kill-server"], workspace_path);
    match run_adb_cmd(&["start-server"], workspace_path) {
        Ok((code, stdout, stderr)) => json!({
            "success": code == 0,
            "action": "restart_server",
            "stdout": stdout.trim(),
            "stderr": stderr.trim(),
        }).to_string(),
        Err(e) => json!({ "success": false, "error": e }).to_string(),
    }
}

/// (serial, state) for every line of `adb devices`.
fn parse_devices(out: &str) -> Vec<(String, String)> {
    out.lines()
        .skip(1)
        .filter_map(|l| {
            let mut it = l.split_whitespace();
            Some((it.next()?.to_string(), it.next()?.to_string()))
        })
        .collect()
}

/// Wi-Fi IPv4 of the device `serial`: the first of several probes that answers (routes differ between Android versions and vendors).
fn find_device_ip(serial: &str, ws: Option<&Path>) -> Option<String> {
    let probes: [(&[&str], &str); 5] = [
        (&["shell", "ip", "-f", "inet", "addr", "show", "wlan0"], r"inet\s+(\d+\.\d+\.\d+\.\d+)/"),
        (&["shell", "ip", "route", "get", "1.1.1.1"], r"src\s+(\d+\.\d+\.\d+\.\d+)"),
        (&["shell", "getprop", "dhcp.wlan0.ipaddress"], r"^\s*(\d+\.\d+\.\d+\.\d+)\s*$"),
        (&["shell", "ifconfig", "wlan0"], r"inet addr:(\d+\.\d+\.\d+\.\d+)"),
        (&["shell", "ip", "route"], r"wlan\d?\b.*?src\s+(\d+\.\d+\.\d+\.\d+)"),
    ];
    for (args, pat) in probes {
        let mut full = vec!["-s", serial];
        full.extend_from_slice(args);
        if let Ok((_, out, _)) = run_adb_cmd(&full, ws) {
            if let Some(m) = regex::Regex::new(pat).ok().and_then(|re| re.captures(&out)).and_then(|c| c.get(1)) {
                let ip = m.as_str().to_string();
                if !ip.starts_with("127.") && !ip.starts_with("0.") {
                    return Some(ip);
                }
            }
        }
    }
    None
}

pub fn handle_connect_wifi(p: &AndroidParams, workspace_path: Option<&Path>) -> String {
    let port = p.port.unwrap_or(5555);
    let port_str = port.to_string();
    let fail = |msg: String| json!({ "success": false, "error": msg }).to_string();

    // 0. Which device? Wireless debugging starts from a USB-attached one.
    let listing = match run_adb_cmd(&["devices"], workspace_path) {
        Ok((_, out, _)) => out,
        Err(e) => return fail(e),
    };
    let devices = parse_devices(&listing);
    let ready: Vec<&(String, String)> = devices.iter().filter(|d| d.1 == "device").collect();
    let usb = ready.iter().find(|d| !d.0.contains(':') && !d.0.starts_with("emulator-"));
    let serial = match usb {
        Some(d) => d.0.clone(),
        None => {
            // No phone on USB: a remembered Wi-Fi device (a TV) may only have moved to a new address.
            if ready.is_empty() {
                if let Some(msg) = remember::reconnect() {
                    if let Ok((now_ready, _)) = super::touch::list_devices() {
                        if let Some((serial, model)) = now_ready.first() {
                            return json!({ "success": true, "action": "connect_wifi", "endpoint": serial, "model": model, "message": format!("{} - connected over Wi-Fi, no USB needed", msg) }).to_string();
                        }
                    }
                }
            }
            let why = if let Some(d) = devices.iter().find(|d| d.1 == "unauthorized") {
                format!("{} is unauthorized: unlock the phone and accept the 'Allow USB debugging' prompt, then run mysid wifi again.", d.0)
            } else if let Some(d) = devices.iter().find(|d| d.1 == "offline") {
                format!("{} is offline: unplug and replug the cable (or run mysid restart_server).", d.0)
            } else if let Some(d) = ready.first() {
                format!("{} is already connected wirelessly (or is an emulator). To use Wi-Fi, plug the phone in with USB once and run mysid wifi.", d.0)
            } else {
                "no device is attached. Plug the phone in with USB (USB debugging on), check `mysid devices`, then run mysid wifi. This is the one-time step that switches the phone to Wi-Fi debugging.".to_string()
            };
            return fail(why);
        }
    };

    // 1. Enable TCP/IP mode on the chosen device (step 2 in wifi.ps1)
    if let Err(e) = run_adb_cmd(&["-s", &serial, "tcpip", &port_str], workspace_path) {
        return fail(format!("Failed to set adb tcpip {}: {}", port_str, e));
    }

    // IMPORTANT (from wifi.ps1): Wait 2 seconds for ADB daemon to restart on device (prevents 'error: closed')
    std::thread::sleep(std::time::Duration::from_millis(2000));

    // 2. Fetch IP address (step 3 in wifi.ps1); the USB serial may re-enumerate after tcpip, so retry a few times.
    let mut ip = p.ip_address.clone().unwrap_or_default().trim().to_string();
    if ip.is_empty() {
        for attempt in 0..3 {
            if let Some(found) = find_device_ip(&serial, workspace_path) {
                ip = found;
                break;
            }
            if attempt < 2 {
                std::thread::sleep(std::time::Duration::from_millis(1500));
            }
        }
    }

    if ip.is_empty() {
        return fail(format!(
            "Device {} is attached, but its Wi-Fi IP could not be read (Wi-Fi off, or on mobile data / VPN only). Turn Wi-Fi on, or pass the address yourself: mysid wifi --ip <phone-ip>",
            serial
        ));
    }

    // Step 4 in wifi.ps1: Pause 500ms for ADB stabilization
    std::thread::sleep(std::time::Duration::from_millis(500));

    // Step 5 in wifi.ps1: Connect via WiFi
    let endpoint = format!("{}:{}", ip, port_str);
    match run_adb_cmd(&["connect", &endpoint], workspace_path) {
        Ok((code, stdout, stderr)) => {
            let ok = code == 0 || stdout.to_lowercase().contains("connected to") || stdout.to_lowercase().contains("already connected");
            if ok {
                remember::save(&endpoint, &remember::model_of(&endpoint));
            }
            json!({
                "success": ok,
                "action": "connect_wifi",
                "endpoint": endpoint,
                "ip": ip,
                "port": port,
                "message": format!("Connected to {} via Wi-Fi", endpoint),
                "stdout": stdout.trim(),
                "stderr": stderr.trim(),
            }).to_string()
        }
        Err(e) => json!({ "success": false, "error": e }).to_string(),
    }
}
