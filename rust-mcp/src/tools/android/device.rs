use serde_json::json;
use std::path::Path;

use super::adb::run_adb_cmd;
use super::AndroidParams;

pub fn handle_devices(workspace_path: Option<&Path>) -> String {
    match run_adb_cmd(&["devices", "-l"], workspace_path) {
        Ok((code, stdout, stderr)) => json!({
            "success": code == 0,
            "action": "devices",
            "exit_code": code,
            "stdout": stdout.trim(),
            "stderr": stderr.trim(),
        }).to_string(),
        Err(e) => json!({ "success": false, "error": e }).to_string(),
    }
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

pub fn handle_connect_wifi(p: &AndroidParams, workspace_path: Option<&Path>) -> String {
    let port = p.port.unwrap_or(5555);
    let port_str = port.to_string();

    // 1. Enable TCP/IP mode on specified port (step 2 in wifi.ps1)
    if let Err(e) = run_adb_cmd(&["tcpip", &port_str], workspace_path) {
        return json!({ "success": false, "error": format!("Failed to set adb tcpip {}: {}", port_str, e) }).to_string();
    }

    // IMPORTANT (from wifi.ps1): Wait 2 seconds for ADB daemon to restart on device (prevents 'error: closed')
    std::thread::sleep(std::time::Duration::from_millis(2000));

    // 2. Fetch IP address (step 3 in wifi.ps1)
    let mut ip = p.ip_address.clone().unwrap_or_default().trim().to_string();

    if ip.is_empty() {
        // Run: adb shell ip route
        if let Ok((code, out, _)) = run_adb_cmd(&["shell", "ip", "route"], workspace_path) {
            if code == 0 {
                // Match regex: 'src\s+([\d\.]+)' (exact logic from wifi.ps1)
                let re = regex::Regex::new(r"src\s+([\d\.]+)").unwrap();
                if let Some(caps) = re.captures(&out) {
                    if let Some(m) = caps.get(1) {
                        ip = m.as_str().trim().to_string();
                    }
                }
            }
        }
    }

    // Fallback: Check wlan0 inet addr directly if ip route did not match
    if ip.is_empty() {
        if let Ok((code, out, _)) = run_adb_cmd(&["shell", "ip", "-f", "inet", "addr", "show", "wlan0"], workspace_path) {
            if code == 0 {
                let re = regex::Regex::new(r"inet\s+([\d\.]+)/").unwrap();
                if let Some(caps) = re.captures(&out) {
                    if let Some(m) = caps.get(1) {
                        ip = m.as_str().trim().to_string();
                    }
                }
            }
        }
    }

    if ip.is_empty() {
        return json!({
            "success": false,
            "error": "Could not detect device Wi-Fi IP address. Please ensure device is connected to Wi-Fi."
        }).to_string();
    }

    // Step 4 in wifi.ps1: Pause 500ms for ADB stabilization
    std::thread::sleep(std::time::Duration::from_millis(500));

    // Step 5 in wifi.ps1: Connect via WiFi
    let endpoint = format!("{}:{}", ip, port_str);
    match run_adb_cmd(&["connect", &endpoint], workspace_path) {
        Ok((code, stdout, stderr)) => {
            let ok = code == 0 || stdout.to_lowercase().contains("connected to") || stdout.to_lowercase().contains("already connected");
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
