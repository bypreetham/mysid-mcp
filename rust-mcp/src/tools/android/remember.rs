//! The last Wi-Fi device, remembered between sessions: a TV gets a new address from the router now and then
//! (192.168.0.102 -> 192.168.0.101), and `devices` / `connect_wifi` then find it again by asking the saved address first and,
//! failing that, scanning the same subnet for the adb port and matching the model.

use super::touch::adb;
use serde_json::{json, Value};
use std::net::{SocketAddr, TcpStream};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

/// Longest any discovery step waits for the device (adb connect, model query).
const STEP: Duration = Duration::from_secs(4);
const PROBE: Duration = Duration::from_millis(350);

fn file() -> PathBuf {
    let base = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("XDG_STATE_HOME").map(PathBuf::from))
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local").join("state")))
        .unwrap_or_else(std::env::temp_dir);
    base.join("mysid").join("android_last_device.json")
}

#[derive(Clone, Debug, PartialEq)]
pub(super) struct Known {
    pub(super) endpoint: String,
    pub(super) model: String,
}

pub(super) fn load() -> Option<Known> {
    let v: Value = serde_json::from_str(&std::fs::read_to_string(file()).ok()?).ok()?;
    Some(Known { endpoint: v.get("endpoint")?.as_str()?.to_string(), model: v.get("model").and_then(|m| m.as_str()).unwrap_or("").to_string() })
}

/// Remembers a device reached over Wi-Fi (serial `ip:port`); USB devices have no address to remember.
pub(super) fn save(serial: &str, model: &str) {
    if !serial.contains(':') || serial.starts_with("emulator") {
        return;
    }
    let path = file();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(path, json!({ "endpoint": serial, "model": model }).to_string());
}

/// `192.168.0.101:5555` -> ("192.168.0.101", 5555)
pub(super) fn split(endpoint: &str) -> Option<(String, u16)> {
    let (ip, port) = endpoint.rsplit_once(':')?;
    Some((ip.to_string(), port.parse().ok()?))
}

/// `adb devices -l` prints the model with every odd character as `_` (MiTV_AXSO2) while `getprop` has the real one (MiTV-AXSO2).
fn same_model(a: &str, b: &str) -> bool {
    let norm = |s: &str| s.chars().map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '_' }).collect::<String>();
    norm(a) == norm(b)
}

fn connect(endpoint: &str) -> bool {
    adb(None, &["connect", endpoint], STEP).map_or(false, |o| {
        let out = String::from_utf8_lossy(&o.stdout).to_lowercase();
        out.contains("connected to") && !out.contains("cannot") && !out.contains("failed")
    })
}

pub(super) fn model_of(endpoint: &str) -> String {
    adb(Some(endpoint), &["shell", "getprop", "ro.product.model"], STEP).map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string()).unwrap_or_default()
}

/// Hosts on the /24 of `ip` that accept a connection on `port`, found with short parallel probes (about a second in all).
fn scan(ip: &str, port: u16) -> Vec<String> {
    let Some((prefix, _)) = ip.rsplit_once('.') else { return Vec::new() };
    let hits = Arc::new(Mutex::new(Vec::new()));
    let mut handles = Vec::new();
    for host in 1..255u32 {
        let (hits, addr) = (hits.clone(), format!("{}.{}:{}", prefix, host, port));
        handles.push(thread::spawn(move || {
            if let Ok(a) = addr.parse::<SocketAddr>() {
                if TcpStream::connect_timeout(&a, PROBE).is_ok() {
                    hits.lock().unwrap().push(addr);
                }
            }
        }));
    }
    for h in handles {
        let _ = h.join();
    }
    let found = hits.lock().unwrap().clone();
    found
}

/// Brings the remembered device back: its saved address first, then the same subnet. Returns a line saying what happened.
pub(super) fn reconnect() -> Option<String> {
    let known = load()?;
    if connect(&known.endpoint) {
        return Some(format!("reconnected {} ({})", known.endpoint, known.model));
    }
    let (ip, port) = split(&known.endpoint)?;
    for endpoint in scan(&ip, port).into_iter().filter(|e| *e != known.endpoint) {
        if !connect(&endpoint) {
            continue;
        }
        let model = model_of(&endpoint);
        if known.model.is_empty() || same_model(&model, &known.model) {
            save(&endpoint, &model);
            return Some(format!("{} ({}) has a new address: {} -> {}", model, known.endpoint, known.endpoint, endpoint));
        }
        let _ = adb(None, &["disconnect", &endpoint], STEP);
    }
    Some(format!("{} ({}) did not answer here or on the rest of {}.x; is it on and on the same Wi-Fi?", if known.model.is_empty() { "the last device" } else { &known.model }, known.endpoint, ip.rsplit_once('.').map_or("", |p| p.0)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_endpoint_splits_into_address_and_port() {
        assert_eq!(split("192.168.0.101:5555"), Some(("192.168.0.101".to_string(), 5555)));
        assert_eq!(split("R58M123ABC"), None);
    }

    #[test]
    fn the_model_matches_whatever_way_adb_spells_it() {
        assert!(same_model("MiTV_AXSO2", "MiTV-AXSO2"));
        assert!(same_model("Pixel 6", "Pixel_6"));
        assert!(!same_model("MiTV-AXSO2", "MiTV-AXSO3"));
    }

    #[test]
    fn only_wifi_serials_are_remembered() {
        // a USB serial and an emulator have no address to come back to
        save("R58M123ABC", "Pixel");
        save("emulator-5554", "sdk");
        assert!(load().map_or(true, |k| k.endpoint != "R58M123ABC" && k.endpoint != "emulator-5554"));
    }
}
